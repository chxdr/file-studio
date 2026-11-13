// MockCore.swift — CoreAPI with no Rust, for UI work (`-FSMockCore`).
//
// A synthetic tree under /Mock (it never touches the disk):
//   /Mock                  home
//   ├─ Applications/       Thing.app (a package), Other.app
//   ├─ Documents/          40 files, case twins (README.md / readme.md),
//   │                      unicode names (NFC "Café" and NFD "Café"), emoji
//   ├─ Downloads/          a few files and a .hidden one
//   ├─ Huge 100k/          100,000 files, generated on demand
//   ├─ iCloud Drive/       dataless placeholders (draw the cloud badge)
//   └─ Pictures/2026/…     nested folders for completion
// A fake operation queue runs ops with progress; pasting onto an existing
// name raises a conflict. Completion matches children and a small frecency
// list. Key handling covers the common Standard keys so the UI can be driven.
//
// Behaviour here only imitates the core so views have something to draw.
// Never copy logic from this file into the app: the real rules are in Rust.

import Foundation

// MARK: - Tree

private final class Node {
    let name: String
    let isDir: Bool
    var isPackage = false
    var isDataless = false
    var size: UInt64
    var mtimeNs: Int64
    /// Generated children: `count` files named by `gen(i)` (already sorted).
    var generated: (count: Int, gen: (Int) -> String)?
    var kids: [Node] = []

    init(_ name: String, dir: Bool = false, size: UInt64 = 0, ageDays: Double = 1) {
        self.name = name; self.isDir = dir; self.size = size
        mtimeNs = Int64((Date().timeIntervalSince1970 - ageDays * 86_400) * 1e9)
    }

    func child(_ n: String) -> Node? {
        if let g = generated {
            // Generated names are "file 000123.dat"; look one up by parsing.
            if let i = Int(n.dropFirst(5).prefix(6)), i < g.count, g.gen(i) == n { return generated(i) }
            return nil
        }
        return kids.first { $0.name == n }
    }

    func generated(_ i: Int) -> Node {
        let n = Node(generated!.gen(i), size: UInt64(1000 + (i * 7919) % 900_000), ageDays: Double(i % 400))
        return n
    }

    var childCount: Int { generated?.count ?? kids.count }
}

private func file(_ n: String, _ kb: UInt64 = 12, age: Double = 3) -> Node { Node(n, size: kb * 1024, ageDays: age) }
private func dir(_ n: String, _ kids: [Node] = []) -> Node { let d = Node(n, dir: true); d.kids = kids; return d }

private func buildTree() -> Node {
    let thing = dir("Thing.app", [dir("Contents", [file("Info.plist", 2)])]); thing.isPackage = true
    let other = dir("Other.app", [dir("Contents")]); other.isPackage = true
    var docs: [Node] = (1...40).map { file(String(format: "Note %02d.txt", $0), UInt64($0), age: Double($0)) }
    docs += [file("README.md"), file("readme.md"), dir("Café"), dir("Cafe\u{301} (NFD)"),
             file("日本語のファイル.txt"), file("emoji 🎉.txt"), file("Naïve résumé.pdf", 240)]
    let huge = dir("Huge 100k")
    huge.generated = (100_000, { String(format: "file %06d.dat", $0) })
    let cloud = dir("iCloud Drive", ["Report.pages", "Budget.numbers", "Slides.key", "Photo.heic"].map {
        let f = file($0, 2048); f.isDataless = true; return f
    })
    let pics = dir("Pictures", [dir("2026", [dir("2026-11-11_gig"), dir("2026-10-02_wedding"), dir("Holiday")]),
                                dir("2025", [dir("Archive")])])
    return dir("Mock", [
        dir("Applications", [thing, other]),
        dir("Documents", docs),
        dir("Downloads", [file("installer.dmg", 90_000), file("photo.jpg", 3_000), file(".hidden-file")]),
        huge, cloud, pics,
        dir("Desktop", [file("todo.txt")]),
    ])
}

// MARK: - Engine

final class MockEngine: EngineAPI, @unchecked Sendable {
    let implementationName = "Mock"
    fileprivate let root = buildTree()
    fileprivate var handler: (@Sendable (CoreEvent) -> Void)?
    private var nextID: UInt64 = 1
    fileprivate var tabs: [TabID: MockTab] = [:]
    private var workspace = WorkspaceState()
    private var settingsValue = Settings()
    // "Old Project" doesn't exist: it shows how a favourite whose folder has gone looks.
    private var favs: [Favourite] = ["Applications", "Desktop", "Documents", "Downloads", "", "Old Project"].map {
        Favourite(path: $0.isEmpty ? "/Mock" : "/Mock/\($0)", name: $0.isEmpty ? "Mock" : $0)
    }
    fileprivate var frecency: [String: Double] = ["/Mock/Pictures/2026/2026-11-11_gig": 9, "/Mock/Documents": 5,
                                                  "/Mock/Downloads": 4, "/Mock/Huge 100k": 2]
    fileprivate var clipboard: [String] = []
    private var ops: [OpStatus] = []
    private var nextOp: OpID = 1
    private var undoStack: [String] = []
    private var closed: [String] = []

    init() {}

    func setEventHandler(_ h: @escaping @Sendable (CoreEvent) -> Void) { handler = h }
    func home() -> String { "/Mock" }

    fileprivate func emit(_ e: CoreEvent) {
        let h = handler
        DispatchQueue.main.async { h?(e) }
    }
    fileprivate func newID() -> UInt64 { defer { nextID += 1 }; return nextID }

    fileprivate func node(_ path: String) -> Node? {
        guard path == "/Mock" || path.hasPrefix("/Mock/") else { return nil }
        var n = root
        for comp in path.split(separator: "/").dropFirst() {
            guard let c = n.child(String(comp)) else { return nil }
            n = c
        }
        return n
    }

    // Settings
    func settings() -> Settings { settingsValue }
    func updateSettings(_ s: Settings) throws { settingsValue = s; emit(.settingsChanged) }
    func setting(_ key: String) throws -> String { throw CoreError(.notAvailable, "Mock: no JSON settings") }
    func setSetting(_ key: String, json: String) throws { throw CoreError(.notAvailable, "Mock: no JSON settings") }

    // Workspace
    private func makeTab(_ path: String) -> MockTab {
        let t = MockTab(engine: self, id: newID(), location: path)
        tabs[t.id] = t
        t.load()
        return t
    }

    func restoreSession() -> WorkspaceState {
        if workspace.windows.isEmpty { _ = try? newWindow(path: nil) }
        return workspace
    }
    func workspaceState() -> WorkspaceState {
        var w = workspace
        for wi in w.windows.indices {
            for pi in w.windows[wi].panes.indices {
                w.windows[wi].panes[pi].tabs = w.windows[wi].panes[pi].tabs.map { tabs[$0.id]?.state() ?? $0 }
            }
        }
        return w
    }
    func tab(_ id: TabID) -> TabAPI? { tabs[id] }
    func activeTab() -> TabAPI? {
        guard let w = workspace.windows.first(where: { $0.id == workspace.activeWindow }) ?? workspace.windows.first,
              let id = w.activeTabID else { return nil }
        return tabs[id]
    }
    private func locate(_ tab: TabID) -> (w: Int, p: Int, t: Int)? {
        for (wi, w) in workspace.windows.enumerated() {
            for (pi, p) in w.panes.enumerated() {
                if let ti = p.tabs.firstIndex(where: { $0.id == tab }) { return (wi, pi, ti) }
            }
        }
        return nil
    }
    func otherPaneTab(of: TabID) -> TabAPI? {
        guard let l = locate(of), workspace.windows[l.w].panes.count == 2 else { return nil }
        return workspace.windows[l.w].panes[1 - l.p].activeTabID.flatMap { tabs[$0] }
    }
    func newWindow(path: String?) throws -> WindowID {
        let t = makeTab(path ?? "/Mock")
        let w = WindowState(id: newID(), panes: [PaneState(id: newID(), tabs: [t.state()], activeTab: 0)], activePane: 0)
        workspace.windows.append(w)
        workspace.activeWindow = w.id
        emit(.tabsChanged)
        return w.id
    }
    func closeWindow(_ id: WindowID) throws {
        workspace.windows.removeAll { $0.id == id }
        workspace.activeWindow = workspace.windows.first?.id
        emit(.tabsChanged)
    }
    func openTab(path: String?) throws -> TabAPI {
        guard let w = workspace.windows.firstIndex(where: { $0.id == workspace.activeWindow }) ?? workspace.windows.indices.first
        else { throw CoreError(.notAvailable, "no window") }
        let pane = workspace.windows[w].panes[workspace.windows[w].activePane].id
        return try openTab(inPane: pane, path: path, index: nil)
    }
    func openTab(inPane pane: PaneID, path: String?, index: UInt32?) throws -> TabAPI {
        for wi in workspace.windows.indices {
            if let pi = workspace.windows[wi].panes.firstIndex(where: { $0.id == pane }) {
                let start = path ?? activeTab()?.location() ?? "/Mock"
                let t = makeTab(start)
                var p = workspace.windows[wi].panes[pi]
                let at = min(Int(index ?? UInt32(p.tabs.count)), p.tabs.count)
                p.tabs.insert(t.state(), at: at)
                p.activeTab = at
                workspace.windows[wi].panes[pi] = p
                emit(.tabsChanged)
                return t
            }
        }
        throw CoreError(.notFound, "pane \(pane)")
    }
    func closeTab(_ id: TabID) throws {
        guard let l = locate(id) else { throw CoreError(.notFound, "tab \(id)") }
        closed.append(tabs[id]?.location() ?? "/Mock")
        var p = workspace.windows[l.w].panes[l.p]
        p.tabs.remove(at: l.t)
        tabs[id] = nil
        if p.tabs.isEmpty {
            workspace.windows[l.w].panes.remove(at: l.p)
            if workspace.windows[l.w].panes.isEmpty { workspace.windows.remove(at: l.w) }
            else { workspace.windows[l.w].activePane = 0 }
        } else {
            p.activeTab = min(p.activeTab, p.tabs.count - 1)
            workspace.windows[l.w].panes[l.p] = p
        }
        emit(.tabsChanged)
    }
    func closeOtherTabs(keep: TabID) throws {
        guard let l = locate(keep) else { return }
        for t in workspace.windows[l.w].panes[l.p].tabs where t.id != keep { try closeTab(t.id) }
    }
    func reopenClosedTab() throws -> TabAPI? {
        guard let p = closed.popLast() else { return nil }
        return try openTab(path: p)
    }
    func activateTab(_ id: TabID) throws {
        guard let l = locate(id) else { throw CoreError(.notFound, "tab \(id)") }
        workspace.windows[l.w].panes[l.p].activeTab = l.t
        workspace.windows[l.w].activePane = l.p
        workspace.activeWindow = workspace.windows[l.w].id
        emit(.tabsChanged)
    }
    func moveTab(_ id: TabID, toPane pane: PaneID, index: UInt32) throws {
        guard let l = locate(id) else { throw CoreError(.notFound, "tab \(id)") }
        guard let wi = workspace.windows.firstIndex(where: { $0.panes.contains { $0.id == pane } }),
              let pi = workspace.windows[wi].panes.firstIndex(where: { $0.id == pane }) else {
            throw CoreError(.notFound, "pane \(pane)")
        }
        let same = l.w == wi && l.p == pi
        if !same && workspace.windows[l.w].panes[l.p].tabs.count == 1 {
            throw CoreError(.notAvailable, "Mock: a pane keeps its last tab")
        }
        let t = workspace.windows[l.w].panes[l.p].tabs.remove(at: l.t)
        if !same {
            let src = workspace.windows[l.w].panes[l.p]
            workspace.windows[l.w].panes[l.p].activeTab = min(src.activeTab, src.tabs.count - 1)
        }
        var dest = workspace.windows[wi].panes[pi]
        let at = min(Int(index), dest.tabs.count)
        dest.tabs.insert(t, at: at)
        dest.activeTab = at
        workspace.windows[wi].panes[pi] = dest
        workspace.windows[wi].activePane = pi
        emit(.tabsChanged)
    }
    func focusPane(window: WindowID, index: UInt32) throws {
        guard let w = workspace.windows.firstIndex(where: { $0.id == window }) else { return }
        workspace.windows[w].activePane = min(Int(index), workspace.windows[w].panes.count - 1)
        emit(.tabsChanged)
    }
    func toggleDual(window: WindowID) throws -> Bool {
        guard let w = workspace.windows.firstIndex(where: { $0.id == window }) else { return false }
        if workspace.windows[w].panes.count == 2 {
            workspace.windows[w].panes.removeLast()
            workspace.windows[w].activePane = 0
        } else {
            let t = makeTab("/Mock/Downloads")
            workspace.windows[w].panes.append(PaneState(id: newID(), tabs: [t.state()], activeTab: 0))
        }
        emit(.tabsChanged)
        return workspace.windows[w].panes.count == 2
    }
    func setLayout(window: WindowID, orientation: Orientation, splitRatio: Double) throws {
        guard let w = workspace.windows.firstIndex(where: { $0.id == window }) else { return }
        workspace.windows[w].orientation = orientation
        workspace.windows[w].splitRatio = splitRatio
        emit(.tabsChanged)
    }
    func swapPanes(window: WindowID) throws {
        guard let w = workspace.windows.firstIndex(where: { $0.id == window }), workspace.windows[w].panes.count == 2 else { return }
        workspace.windows[w].panes.reverse()
        emit(.tabsChanged)
    }
    // Workspaces: names and icons only (the mock keeps one set of windows).
    private var mockWorkspaces: [WorkspaceSummary] = [
        WorkspaceSummary(id: 1, name: "Everyday", icon: "house", active: true, windows: 1, tabs: 1, updatedNs: 0),
        WorkspaceSummary(id: 2, name: "Photo work", icon: "camera", windows: 1, tabs: 3, updatedNs: 0),
    ]
    private func renumberWorkspaces() {
        for i in mockWorkspaces.indices {
            mockWorkspaces[i].position = UInt32(i)
            mockWorkspaces[i].shortcut = i < 9 ? "⌃⌘\(i + 1)" : nil
        }
        emit(.workspacesChanged)
    }
    func saveWorkspace(name: String) throws -> WorkspaceID { try createWorkspace(name: name, icon: "", copyCurrent: true) }
    func savedWorkspaces() throws -> [WorkspaceSummary] { try workspaces() }
    func openWorkspace(_ id: WorkspaceID) throws -> WorkspaceState { workspaceState() }
    func renameWorkspace(_ id: WorkspaceID, name: String) throws {
        guard let i = mockWorkspaces.firstIndex(where: { $0.id == id }) else { return }
        mockWorkspaces[i].name = name
        renumberWorkspaces()
    }
    func deleteWorkspace(_ id: WorkspaceID) throws {
        guard mockWorkspaces.count > 1 else { return }
        let wasActive = mockWorkspaces.first { $0.id == id }?.active ?? false
        mockWorkspaces.removeAll { $0.id == id }
        if wasActive { mockWorkspaces[0].active = true }
        renumberWorkspaces()
    }
    func workspaces() throws -> [WorkspaceSummary] {
        if mockWorkspaces.first?.shortcut == nil { renumberWorkspaces() }
        return mockWorkspaces
    }
    func workspaceIcons() -> [String] { ["house", "sun.max", "camera", "photo.on.rectangle", "film", "briefcase"] }
    func createWorkspace(name: String, icon: String, copyCurrent: Bool) throws -> WorkspaceID {
        let id = (mockWorkspaces.map(\.id).max() ?? 0) + 1
        mockWorkspaces.append(WorkspaceSummary(id: id, name: name, icon: icon.isEmpty ? "square.grid.2x2" : icon,
                                               windows: 1, tabs: 1, updatedNs: 0))
        _ = try switchWorkspace(id)
        return id
    }
    func switchWorkspace(_ id: WorkspaceID) throws -> WorkspaceState {
        for i in mockWorkspaces.indices { mockWorkspaces[i].active = mockWorkspaces[i].id == id }
        renumberWorkspaces()
        return workspaceState()
    }
    func setWorkspaceIcon(_ id: WorkspaceID, icon: String) throws {
        guard let i = mockWorkspaces.firstIndex(where: { $0.id == id }) else { return }
        mockWorkspaces[i].icon = icon
        renumberWorkspaces()
    }
    func moveWorkspace(_ id: WorkspaceID, to index: UInt32) throws {
        guard let i = mockWorkspaces.firstIndex(where: { $0.id == id }) else { return }
        let w = mockWorkspaces.remove(at: i)
        mockWorkspaces.insert(w, at: min(Int(index), mockWorkspaces.count))
        renumberWorkspaces()
    }
    func duplicateWorkspace(_ id: WorkspaceID) throws -> WorkspaceID {
        guard let i = mockWorkspaces.firstIndex(where: { $0.id == id }) else { return 0 }
        var w = mockWorkspaces[i]
        w.id = (mockWorkspaces.map(\.id).max() ?? 0) + 1
        w.name += " copy"
        w.active = false
        mockWorkspaces.insert(w, at: i + 1)
        renumberWorkspaces()
        return w.id
    }
    func askDeleteWorkspace(_ id: WorkspaceID) throws -> Outcome {
        try deleteWorkspace(id)
        return Outcome()
    }
    func setSidebar(window: WindowID, width: Double, hidden: Bool) throws {
        guard let w = workspace.windows.firstIndex(where: { $0.id == window }) else { return }
        workspace.windows[w].sidebarWidth = width
        workspace.windows[w].sidebarHidden = hidden
    }

    // MARK: Commands

    private static let table: [(id: String, title: String, cat: String, keys: String?)] = [
        ("nav.open", "Open", "Navigation", "⌘↓"), ("nav.up", "Enclosing Folder", "Navigation", "⌘↑"),
        ("nav.back", "Back", "Navigation", "⌘["), ("nav.forward", "Forward", "Navigation", "⌘]"),
        ("nav.address_bar", "Go to Address Bar", "Navigation", "⌘L|/"), ("nav.go_home", "Go to Home", "Navigation", "⇧⌘H"),
        ("select.all", "Select All", "Selection", "⌘A"), ("file.rename", "Rename", "File", "↩"),
        ("file.new_folder", "New Folder", "File", "⇧⌘N"), ("file.trash", "Move to Trash", "File", "⌘⌫"),
        ("file.duplicate", "Duplicate", "File", "⌘D"), ("edit.copy", "Copy", "Edit", "⌘C"),
        ("edit.paste", "Paste", "Edit", "⌘V"), ("edit.undo", "Undo", "Edit", "⌘Z"),
        ("view.show_hidden", "Show Hidden Files", "View", "⇧⌘."), ("view.list", "as List", "View", "⌘2"),
        ("view.columns", "as Columns", "View", "⌘3"), ("tab.new", "New Tab", "Tabs & Windows", "⌘T"),
        ("tab.close", "Close Tab", "Tabs & Windows", "⌘W"), ("pane.toggle_dual", "Toggle Dual Pane", "Panes", "⌘\\"),
        ("preview.quick_look", "Quick Look", "Quick Look & Info", "Space"),
        ("preview.get_info", "Get Info", "Quick Look & Info", "⌘I"),
        ("path.copy_path", "Copy Path", "Paths & Open In", "⌥⌘C"), ("terminal.open", "Open in Terminal", "Paths & Open In", "⇧⌘R"),
        ("app.settings", "Settings…", "Settings", "⌘,"),
        ("app.keyboard_shortcuts", "Keyboard Shortcuts…", "Settings", nil),
        ("app.power_tour", "File Studio Power Tour", "Settings", nil),
        ("favourites.add", "Add to Favourites", "Navigation", "⌃⌘T"),
        ("addressbar.complete_next", "Complete / Next Suggestion", "Text Fields", "⇥"),
        ("addressbar.commit", "Go", "Text Fields", "↩"),
        ("edit.redo", "Redo", "Edit", "⇧⌘Z"),
        ("filter.clear", "Clear Filter", "Palette & Filter", "⎋"),
        ("favourites.remove", "Remove from Favourites", "Navigation", nil),
        ("file.copy_to_other_pane", "Copy to Other Pane", "File", "F5"),
        ("file.move_to_other_pane", "Move to Other Pane", "File", "F6"),
        ("pane.switch", "Switch Pane", "Panes", "⇥"),
        ("edit.cut", "Cut", "Edit", "⌘X"),
        ("filter.focus", "Filter List", "Palette & Filter", "⌘F"),
        ("palette.open", "Command Palette…", "Palette & Filter", "⇧⌘P"),
    ]

    func commands() -> [CommandInfo] {
        Self.table.map {
            CommandInfo(id: $0.id, title: $0.title, category: $0.cat, description: $0.title,
                        shortcut: $0.keys?.components(separatedBy: "|").first,
                        showsInMenu: true, showsInPalette: true, customized: false,
                        handling: ["preview.quick_look", "preview.get_info", "path.copy_path", "terminal.open",
                                   "app.settings", "app.keyboard_shortcuts", "app.power_tour", "file.rename",
                                   "nav.address_bar"].contains($0.id) ? .shell : .core)
        }
    }

    private func keyCommand(_ c: KeyChord, focus: Focus) -> Invocation? {
        let m = c.mods
        if focus == .addressBar {
            // Mirrors the Standard keymap's AddressBarEditing bindings.
            switch (c.key, m.shift, m.cmd, m.alt) {
            case (.returnKey, false, false, false): return Invocation(id: "addressbar.commit")
            case (.returnKey, false, true, false): return Invocation(id: "addressbar.commit_other_pane")
            case (.escape, false, false, false): return Invocation(id: "addressbar.cancel")
            case (.tab, false, false, false): return Invocation(id: "addressbar.complete_next")
            case (.tab, true, false, false): return Invocation(id: "addressbar.complete_prev")
            case (.down, false, false, false): return Invocation(id: "addressbar.next")
            case (.up, false, false, false): return Invocation(id: "addressbar.prev")
            case (.end, false, false, false): return Invocation(id: "addressbar.accept_ghost")
            case (.delete, false, false, true): return Invocation(id: "addressbar.delete_component")
            case (.delete, false, true, false): return Invocation(id: "addressbar.clear_line")
            case (.char("d"), false, true, false): return Invocation(id: "addressbar.add_favourite")
            case (.char("l"), false, true, false), (.char("g"), true, true, false): return Invocation(id: "nav.address_bar")
            default: return nil
            }
        }
        if m.ctrl, m.cmd, !m.shift, !m.alt, c.key == .char("t") { return Invocation(id: "favourites.add") }
        if m.ctrl { return nil }
        switch (c.key, m.cmd, m.shift, m.alt) {
        case (.up, true, false, false): return Invocation(id: "nav.up")
        case (.down, true, false, false), (.char("o"), true, false, false): return Invocation(id: "nav.open")
        case (.char("["), true, _, _): return Invocation(id: "nav.back")
        case (.char("]"), true, _, _): return Invocation(id: "nav.forward")
        case (.char("l"), true, false, false), (.char("g"), true, true, false): return Invocation(id: "nav.address_bar")
        case (.char("/"), false, false, false) where focus == .list:
            return Invocation(id: "nav.address_bar", arg: .char("/"))
        case (.char("`"), false, true, false) where focus == .list:
            return Invocation(id: "nav.address_bar", arg: .char("~"))
        case (.char("h"), true, true, false): return Invocation(id: "nav.go_home")
        case (.char("a"), true, false, false): return Invocation(id: "select.all")
        case (.char("n"), true, true, false): return Invocation(id: "file.new_folder")
        case (.delete, true, false, false): return Invocation(id: "file.trash")
        case (.char("d"), true, false, false): return Invocation(id: "file.duplicate")
        case (.char("c"), true, false, false): return Invocation(id: "edit.copy")
        case (.char("c"), true, false, true): return Invocation(id: "path.copy_path")
        case (.char("v"), true, false, false): return Invocation(id: "edit.paste")
        case (.char("z"), true, false, false): return Invocation(id: "edit.undo")
        case (.char("."), true, true, false): return Invocation(id: "view.show_hidden")
        case (.char("2"), true, false, false): return Invocation(id: "view.list")
        case (.char("3"), true, false, false): return Invocation(id: "view.columns")
        case (.char("t"), true, false, false): return Invocation(id: "tab.new")
        case (.char("w"), true, false, false): return Invocation(id: "tab.close")
        case (.char("\\"), true, false, false): return Invocation(id: "pane.toggle_dual")
        case (.char("i"), true, false, false): return Invocation(id: "preview.get_info")
        case (.char("r"), true, true, false): return Invocation(id: "terminal.open")
        case (.char(","), true, false, false): return Invocation(id: "app.settings")
        case (.space, false, false, false): return Invocation(id: "preview.quick_look")
        case (.returnKey, false, false, false): return Invocation(id: "file.rename")
        default: return nil
        }
    }

    func handleKey(_ chord: KeyChord, context: InputContext) -> Outcome? {
        if let o = columnKey(chord, context: context) { return o }  // ui-list addition
        if let inv = keyCommand(chord, focus: context.focus) {
            // S6: a held key repeats movement only.
            if context.isRepeat && !inv.id.hasPrefix("cursor.") && !inv.id.hasPrefix("select.extend_") { return Outcome() }
            var inv = inv
            inv.tab = context.tab
            return (try? perform(inv, via: .keyboard)) ?? Outcome(command: inv.id, message: "Not available")
        }
        guard context.focus == .list, let t = (context.tab.flatMap { tabs[$0] } ?? activeTab() as? MockTab) else { return nil }
        let m = chord.mods
        switch chord.key {
        case .up, .down:
            t.setSelection(.move(chord.key == .up ? .up : .down, extend: m.shift))
            return Outcome(tab: t.id)
        case .home, .end:
            t.setSelection(.move(chord.key == .home ? .home : .end, extend: m.shift))
            return Outcome(tab: t.id)
        case .char(let ch) where m.isEmpty:
            _ = t.typeSelect(String(ch))
            return Outcome(tab: t.id)
        default:
            return nil
        }
    }
    func pollKeys(context: InputContext) -> Outcome? { nil }
    func pendingKeys() -> String? { nil }

    func perform(_ inv: Invocation, via: Via) throws -> Outcome {
        guard let t = (inv.tab.flatMap { tabs[$0] } ?? activeTab() as? MockTab) else {
            throw CoreError(.notAvailable, "no window is open")
        }
        var out = Outcome(command: inv.id, tab: t.id)
        let sel = t.selectedPaths()
        let need = { () throws -> [String] in
            if case .path(let p) = inv.arg { return [p] }
            if sel.isEmpty { throw CoreError(.notAvailable, "nothing is selected") }
            return sel
        }
        switch inv.id {
        case "nav.open":
            let p = try need()
            if let n = node(p[0]), n.isDir, !n.isPackage { t.navigate(p[0]) } else { out.shell = .open(paths: p) }
        case "nav.up": out = try t.up()
        case "nav.back": out = try t.back()
        case "nav.forward": out = try t.forward()
        case "nav.go_home": t.navigate("/Mock")
        case "nav.address_bar":
            var text = ""
            if case .char(let ch) = inv.arg { text = String(ch) }
            out.shell = .focusAddressBar(pane: paneOf(t.id) ?? 0, text: text)
        case "addressbar.commit":
            if case .path(let p) = inv.arg { out = try t.go(p) } else { out.shell = .textField(command: inv.id) }
        case "addressbar.commit_other_pane":
            if case .path(let p) = inv.arg {
                guard let o = otherPaneTab(of: t.id) as? MockTab else { throw CoreError(.notAvailable, "single pane") }
                out = try o.go(p)
            } else { out.shell = .textField(command: inv.id) }
        case "addressbar.add_favourite":
            if case .path(let p) = inv.arg { try addFavourite(p, index: nil); out.message = "Added to Favourites" }
            else { out.shell = .textField(command: inv.id) }
        case let id where id.hasPrefix("addressbar."): out.shell = .textField(command: id)
        case "select.all": t.setSelection(.all)
        case "file.rename":
            guard let c = t.selection().cursor, let r = t.rows(start: c, count: 1).first else {
                throw CoreError(.notAvailable, "nothing is selected")
            }
            out.shell = .beginRename(tab: t.id, name: r.name)
        case "file.new_folder":
            out.ops = [startOp(.newFolder, "New Folder", files: 1, bytes: 0, into: t.location(), name: "untitled folder")]
            out.shell = .beginRename(tab: t.id, name: "untitled folder")
        case "file.trash": out.ops = [startOp(.trash, "Move to Trash", files: UInt64(try need().count), bytes: 0)]
        case "file.duplicate": out.ops = [startOp(.duplicate, "Duplicate", files: UInt64(try need().count), bytes: 50_000_000)]
        case "edit.copy":
            clipboard = try need()
            out.shell = .writeFilePasteboard(paths: clipboard)
        case "edit.paste", "file.move_here":
            guard !clipboard.isEmpty else { return Outcome(command: inv.id, message: "Nothing to paste") }
            let names = clipboard.map { ($0 as NSString).lastPathComponent }
            let clash = names.first { node(t.location() + "/" + $0) != nil }
            out.ops = [startOp(.copy, "Copy \(names.count) item(s)", files: UInt64(names.count), bytes: 400_000_000,
                               conflictWith: clash.map { t.location() + "/" + $0 })]
        case "edit.undo":
            out.message = undoStack.popLast().map { "Undid \($0)" } ?? "Nothing to undo"
        case "view.show_hidden":
            var v = t.view(); v.showHidden.toggle(); t.setView(v)
        case "view.list", "view.columns", "view.icons", "view.gallery":
            var v = t.view()
            v.mode = inv.id == "view.list" ? .list : inv.id == "view.columns" ? .columns : inv.id == "view.icons" ? .icons : .gallery
            t.setView(v)
        case let id where id.hasPrefix("sort."):
            var v = t.view()
            switch id {
            case "sort.name": v.sort = .name
            case "sort.size": v.sort = .size
            case "sort.date_modified": v.sort = .modified
            case "sort.reverse": v.ascending.toggle()
            case "sort.folders_first": v.foldersFirst.toggle()
            default: break
            }
            t.setView(v)
        case "tab.new": out.tab = try openTab(path: t.location()).id
        case "tab.close": try closeTab(t.id)
        case "pane.toggle_dual":
            if let l = locate(t.id) { out.message = try toggleDual(window: workspace.windows[l.w].id) ? "Dual pane" : "Single pane" }
        case "preview.quick_look": out.shell = .quickLook(paths: try need())
        case "preview.get_info": out.shell = .getInfo(paths: sel.isEmpty ? [t.location()] : sel)
        case "file.reveal_in_finder": out.shell = .revealInFinder(paths: sel.isEmpty ? [t.location()] : sel)
        case let id where id.hasPrefix("path.copy"):
            if case .path(let p) = inv.arg { out.shell = .copyText(p) } else {
                out.shell = .copyText((sel.isEmpty ? [t.location()] : sel).joined(separator: "\n"))
            }
        case "terminal.open":
            if case .path(let p) = inv.arg { out.shell = .openTerminal(dir: p) } else { out.shell = .openTerminal(dir: t.location()) }
        case "app.settings": out.shell = .showSettings(section: "")
        case "app.keyboard_shortcuts": out.shell = .showSettings(section: "shortcuts")
        case "app.power_tour": out.shell = .showPowerTour
        case "favourites.add", "favourites.remove":
            let ps: [String]
            if case .path(let p) = inv.arg { ps = [p] } else { ps = sel.isEmpty ? [t.location()] : sel }
            let folders = ps.filter { node($0)?.isDir == true }
            if inv.id == "favourites.add" {
                let new = folders.filter { f in !favs.contains { $0.path == f } }
                for f in new { try addFavourite(f, index: nil) }
                out.message = folders.isEmpty ? "Only folders can be favourites"
                    : new.isEmpty ? "Already in Favourites"
                    : new.count == 1 ? "Added “\((new[0] as NSString).lastPathComponent)” to Favourites"
                    : "Added \(new.count) folders to Favourites"
            } else {
                let gone = ps.filter { p in favs.contains { $0.path == p } }
                for p in gone { try removeFavourite(p) }
                out.message = gone.isEmpty ? "Not in Favourites" : "Removed \(gone.count == 1 ? "“\((gone[0] as NSString).lastPathComponent)”" : "\(gone.count) folders") from Favourites"
            }
        case "sidebar.rename":
            if case .path(let p) = inv.arg {
                out.shell = .askFavouriteName(path: p, name: favs.first { $0.path == p }?.name ?? (p as NSString).lastPathComponent)
            }
        case "tags.toggle": out.message = "Mock: tags aren't stored"
        case "drive.eject":
            if case .path(let p) = inv.arg { out.shell = .eject(path: p) }
        case "drive.info":
            if case .path(let p) = inv.arg { out.shell = .showDriveInfo(path: p) }
        case "sidebar.remove":
            if case .path(let p) = inv.arg {
                let name = favs.first { $0.path == p }?.name ?? (p as NSString).lastPathComponent
                try removeFavourite(p)
                out.message = "Removed “\(name)” from Favourites"
            }
        case "nav.open_new_tab":
            if case .path(let p) = inv.arg, let l = locate(t.id) {
                out.tab = try openTab(inPane: workspace.windows[l.w].panes[l.p].id, path: p, index: nil).id
            }
        case "pane.equalize":
            if let l = locate(t.id) {
                let w = workspace.windows[l.w]
                try setLayout(window: w.id, orientation: w.orientation, splitRatio: 0.5)
            }
        default: throw CoreError(.unknownCommand, "Mock doesn't do \(inv.id)")
        }
        return out
    }

    private func paneOf(_ tab: TabID) -> PaneID? {
        locate(tab).map { workspace.windows[$0.w].panes[$0.p].id }
    }

    func heldShortcuts(_ mods: Mods, context: InputContext) -> HeldShortcuts? {
        guard mods.cmd else { return nil }
        return HeldShortcuts(title: "⌘ Command", rows: Self.table.compactMap { r in
            guard let k = r.keys?.components(separatedBy: "|").first, k.hasPrefix("⌘"), k.count == 2 else { return nil }
            return ShortcutRow(command: r.id, keys: String(k.dropFirst()), label: r.title, category: r.cat)
        })
    }
    func hintFor(command: String, via: Via) -> Hint? {
        guard via == .mouse, let r = Self.table.first(where: { $0.id == command }),
              let k = r.keys?.components(separatedBy: "|").first else { return nil }
        return Hint(command: command, keys: k, label: r.title)
    }
    func vimMode() -> VimMode { .off }
    func commandKeys(_ command: String) -> [String] {
        Self.table.first { $0.id == command }?.keys?.components(separatedBy: "|") ?? []
    }
    func setCommandKey(_ command: String, primary: Bool, keys: String?) throws {}
    func resetCommandKeys(_ command: String?) throws {}
    func keymapConflicts() -> [String] { [] }
    func renameItem(tab: TabID, oldName: String, newName: String) throws -> Outcome {
        guard !newName.isEmpty, !newName.contains("/") else { throw CoreError(.invalidName, "That name can't be used") }
        return Outcome(tab: tab, ops: [startOp(.rename, "Rename", files: 1, bytes: 0)])
    }
    func createFolderAndGo(tab: TabID, path: String) throws -> Outcome {
        Outcome(tab: tab, message: "Mock: folders aren't created")
    }
    func setClipboard(_ paths: [String], cut: Bool) { clipboard = paths }
    func syncPasteboard(changeCount: Int, files: [String]) {}
    func pasteboardWritten(changeCount: Int) {}
    func dropAction(sources: [String], destDir: String, mods: Mods) -> DropAction {
        guard !sources.isEmpty, node(destDir)?.isDir == true,
              !sources.contains(where: { destDir == $0 || destDir.hasPrefix($0 + "/") }) else { return .none }
        if mods.alt { return .copy }
        if sources.allSatisfy({ ($0 as NSString).deletingLastPathComponent == destDir }) { return .none }
        return .move
    }
    func dropFiles(sources: [String], destDir: String, mods: Mods) throws -> Outcome {
        let a = dropAction(sources: sources, destDir: destDir, mods: mods)
        guard a != .none else { throw CoreError(.notAvailable, "the items can't be dropped there") }
        let name = { (p: String) in (p as NSString).lastPathComponent }
        // F-023, roughly: the mock's standard folders ask first.
        if a == .move, let hit = sources.first(where: { Self.protectedMock.contains($0) }) {
            pendingMoves += 1
            pendingMove[pendingMoves] = (sources, destDir)
            return Outcome(shell: .confirmMove(token: pendingMoves, title: "Move “\(name(hit))” into “\(name(destDir))”?",
                                               detail: "It’s one of your standard folders.", paths: sources))
        }
        var out = Outcome(ops: [startOp(a == .copy ? .copy : .move, a == .copy ? "Copy" : "Move",
                                        files: UInt64(sources.count), bytes: 9_000_000)])
        if a == .move {
            out.shell = .moved(text: sources.count == 1 ? "Moved “\(name(sources[0]))” to “\(name(destDir))”"
                               : "Moved \(sources.count) items to “\(name(destDir))”")
        }
        return out
    }

    private static let protectedMock: Set<String> = ["/Mock", "/Mock/Applications", "/Mock/Desktop", "/Mock/Documents",
                                                     "/Mock/Downloads", "/Mock/Pictures"]
    private var pendingMoves: UInt64 = 0
    private var pendingMove: [UInt64: ([String], String)] = [:]

    func confirmMove(_ token: UInt64) throws -> Outcome {
        guard let (sources, dest) = pendingMove.removeValue(forKey: token) else {
            throw CoreError(.notFound, "pending move \(token)")
        }
        return Outcome(shell: .moved(text: "Moved “\((sources[0] as NSString).lastPathComponent)” to “\((dest as NSString).lastPathComponent)”"),
                       ops: [startOp(.move, "Move", files: UInt64(sources.count), bytes: 9_000_000)])
    }
    func cancelMove(_ token: UInt64) { pendingMove[token] = nil }
    func confirm(_ token: UInt64) throws -> Outcome { try confirmMove(token) }
    func cancelConfirmation(_ token: UInt64) { cancelMove(token) }
    func clipboardPaths() -> [String] { clipboard }

    // MARK: Menus

    func menuTarget(tab: TabID, row: UInt32?) throws -> MenuTarget {
        guard let t = tabs[tab] else { throw CoreError(.notFound, "tab \(tab)") }
        guard let row, row < t.listingInfo().count else { t.setSelection(.none); return .emptySpace(tab: tab) }
        if !t.selection().rows.contains(row) { t.setSelection(.only(row)) }
        let paths = t.selectedPaths()
        if paths.count > 1 { return .multi(tab: tab, paths: paths) }
        guard let n = node(paths[0]) else { return .emptySpace(tab: tab) }
        return n.isPackage ? .package(tab: tab, path: paths[0]) : n.isDir ? .folder(tab: tab, path: paths[0])
            : .file(tab: tab, path: paths[0])
    }

    func contextMenu(_ target: MenuTarget, context: MenuContext) throws -> [MenuItem] {
        func cmd(_ id: String, _ title: String, tab: TabID?, arg: CommandArg? = nil, enabled: Bool = true,
                 checked: Bool = false) -> MenuItem {
            .command(MenuCommand(entry: id, title: title, shortcut: Self.table.first { $0.id == id }?.keys,
                                 enabled: enabled, checked: checked, invocation: Invocation(id: id, arg: arg, tab: tab)))
        }
        let canPaste = !clipboard.isEmpty || context.pasteboardHasFiles
        func copyPath(_ tab: TabID?, _ arg: CommandArg?) -> MenuItem {
            .submenu(entry: "menu.copy_path", title: "Copy Path", items: [
                cmd("path.copy_path", "Path", tab: tab, arg: arg), cmd("path.copy_name", "Name", tab: tab, arg: arg),
                cmd("path.copy_escaped", "Escaped", tab: tab, arg: arg),
                cmd("path.copy_home_relative", "Relative to ~", tab: tab, arg: arg),
            ])
        }
        /// Add to or Remove from Favourites, by whether every folder is one already.
        func favItem(_ folders: [String], tab: TabID?, arg: CommandArg?) -> [MenuItem] {
            let dirs = folders.filter { node($0)?.isDir == true }
            guard !dirs.isEmpty else { return [] }
            let pinned = dirs.allSatisfy { d in favs.contains { $0.path == d } }
            return [pinned ? cmd("favourites.remove", "Remove from Favourites", tab: tab, arg: arg)
                           : cmd("favourites.add", "Add to Favourites", tab: tab, arg: arg)]
        }
        switch target {
        case .file(let t, let p), .folder(let t, let p), .package(let t, let p):
            return [cmd("nav.open", "Open", tab: t), .system(.openWith, title: "Open With", paths: [p]), .separator,
                    cmd("preview.quick_look", "Quick Look", tab: t), cmd("preview.get_info", "Get Info", tab: t), .separator,
                    cmd("file.rename", "Rename", tab: t), cmd("file.duplicate", "Duplicate", tab: t), .separator,
                    cmd("edit.copy", "Copy", tab: t), cmd("edit.paste", "Paste", tab: t, enabled: canPaste), .separator,
                    copyPath(t, nil), .separator,
                    .submenu(entry: "menu.tags", title: "Tags", items: [
                        .command(MenuCommand(entry: "tags.toggle", title: "Red", tagColor: 6,
                                             invocation: Invocation(id: "tags.toggle", arg: .text("Red"), tab: t))),
                    ]),
                    .separator, cmd("file.reveal_in_finder", "Reveal in Finder", tab: t),
                    cmd("terminal.open", "Open in Terminal", tab: t)] + favItem([p], tab: t, arg: .path(p))
                + [.system(.share, title: "Share", paths: [p]), .separator, cmd("file.trash", "Move to Trash", tab: t)]
        case .multi(let t, let ps):
            return [cmd("nav.open", "Open", tab: t), cmd("preview.quick_look", "Quick Look \(ps.count) Items", tab: t),
                    .separator, cmd("edit.copy", "Copy", tab: t), cmd("edit.paste", "Paste", tab: t, enabled: canPaste),
                    .separator] + favItem(ps, tab: t, arg: nil)
                + [.separator, cmd("file.trash", "Move \(ps.count) Items to Trash", tab: t)]
        case .emptySpace(let t):
            let v = tabs[t]?.view() ?? ViewState()
            return [cmd("file.new_folder", "New Folder", tab: t), cmd("edit.paste", "Paste", tab: t, enabled: canPaste),
                    .separator,
                    .submenu(entry: "menu.sort_by", title: "Sort by", items: [
                        cmd("sort.name", "Name", tab: t, checked: v.sort == .name),
                        cmd("sort.size", "Size", tab: t, checked: v.sort == .size),
                        cmd("sort.date_modified", "Date Modified", tab: t, checked: v.sort == .modified),
                    ]),
                    cmd("view.show_hidden", "Show Hidden Files", tab: t, checked: v.showHidden), .separator,
                    cmd("select.all", "Select All", tab: t), cmd("terminal.open", "Open in Terminal", tab: t)]
        case .crumb(let t, let p):
            return [cmd("nav.open", "Open", tab: t, arg: .path(p)), .separator, copyPath(t, .path(p)), .separator,
                    cmd("terminal.open", "Open in Terminal", tab: t, arg: .path(p))] + favItem([p], tab: t, arg: .path(p))
        case .sidebarItem(let p, let k):
            var items = [cmd("nav.open", "Open", tab: nil, arg: .path(p)), cmd("path.copy_path", "Copy Path", tab: nil, arg: .path(p)),
                         MenuItem.separator]
            if k == .favourite {
                items += [cmd("sidebar.remove", "Remove from Favourites", tab: nil, arg: .path(p)),
                          cmd("sidebar.rename", "Rename Favourite…", tab: nil, arg: .path(p))]
            } else {
                items.append(cmd("drive.info", "Show Drive Info", tab: nil, arg: .path(p)))
            }
            if k == .volume, volumes().first(where: { $0.mountPath == p })?.isRemovable == true {
                items.append(cmd("drive.eject", "Eject", tab: nil, arg: .path(p)))
            }
            return items
        case .tab(let t):
            let loc = tabs[t]?.location() ?? "/Mock"
            return [cmd("tab.new", "New Tab", tab: t), .separator, cmd("tab.close", "Close Tab", tab: t), .separator]
                + favItem([loc], tab: t, arg: .path(loc))
        }
    }
    func menuEntries(_ kind: MenuKind) -> [MenuEntry] {
        [MenuEntry(entry: "nav.open", title: "Open"), MenuEntry(entry: "---", title: ""),
         MenuEntry(entry: "file.trash", title: "Move to Trash")]
    }
    func menuChoices(_ kind: MenuKind) -> [MenuEntry] { [MenuEntry(entry: "file.duplicate", title: "Duplicate")] }
    func menuAdd(_ kind: MenuKind, entry: String, after: String?) throws {}
    func menuRemove(_ kind: MenuKind, entry: String) throws {}
    func menuMove(_ kind: MenuKind, entry: String, after: String?) throws {}
    func resetMenu(_ kind: MenuKind) throws {}
    func resetMenus() throws {}

    // MARK: Path bar

    func parseLocation(_ raw: String, cwd: String) -> ParsedLocation {
        var p = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        var chip: String?
        if p.count >= 2, p.hasPrefix("'") || p.hasPrefix("\""), p.last == p.first {
            p = String(p.dropFirst().dropLast()); chip = "Cleaned up paste: removed quotes"
        }
        if p.hasPrefix("~") { p = "/Mock" + p.dropFirst() }
        if !p.hasPrefix("/") { p = cwd + "/" + p }
        while p.count > 1, p.hasSuffix("/") { p.removeLast() }
        guard let n = node(p) else {
            var near = (p as NSString).deletingLastPathComponent
            while node(near) == nil, near.count > 1 { near = (near as NSString).deletingLastPathComponent }
            return ParsedLocation(location: .missing(path: p, nearestExisting: near), cleaned: p, chip: chip)
        }
        let l: Location = n.isDir ? .dir(p) : .file(dir: (p as NSString).deletingLastPathComponent, name: n.name)
        return ParsedLocation(location: l, cleaned: p, chip: chip)
    }

    func breadcrumbs(path: String) -> [Crumb] {
        var out: [Crumb] = [Crumb(name: "Mock HD", path: "/", kind: .volumeRoot, isCurrent: path == "/")]
        var acc = ""
        for comp in path.split(separator: "/") {
            acc += "/" + comp
            let n = node(acc)
            out.append(Crumb(name: String(comp), path: acc,
                             kind: acc == "/Mock" ? .home : (n?.isPackage == true ? .package : .folder),
                             isCurrent: acc == path))
        }
        return out
    }
    func subfolders(path: String) -> [Crumb] {
        guard let n = node(path), n.generated == nil else { return [] }
        return n.kids.filter(\.isDir).map { Crumb(name: $0.name, path: path + "/" + $0.name, kind: $0.isPackage ? .package : .folder,
                                                  isCurrent: false) }
    }
    func complete(_ input: String, cursor: UInt32, cwd: String) -> Completions {
        var c = Completions()
        let text = String(input.utf8.prefix(Int(cursor))) ?? input
        let pathLike = text.contains("/") || text.hasPrefix("~")
        if pathLike {
            let expanded = text.hasPrefix("~") ? "/Mock" + text.dropFirst() : (text.hasPrefix("/") ? text : cwd + "/" + text)
            let parent = expanded.hasSuffix("/") ? String(expanded.dropLast()) : (expanded as NSString).deletingLastPathComponent
            let prefix = expanded.hasSuffix("/") ? "" : (expanded as NSString).lastPathComponent
            c.parent = parent
            c.segmentStart = text.utf8.count - prefix.utf8.count
            if let n = node(parent), n.generated == nil {
                c.items = n.kids.filter { $0.isDir && $0.name.lowercased().hasPrefix(prefix.lowercased()) }.map { k in
                    let full = parent + "/" + k.name
                    return Completion(displayName: k.name, fullPath: full, breadcrumbText: "~" + full.dropFirst(5),
                                      isFrecent: false, matchRanges: [0..<prefix.utf8.count], score: 100 + (frecency[full] ?? 0),
                                      insertText: String(text.dropLast(prefix.count)) + k.name + "/")
                }
            }
        } else if !text.isEmpty {
            c.isBareWord = true
            c.items = frecency.filter { ($0.key as NSString).lastPathComponent.localizedCaseInsensitiveContains(text) }
                .sorted { $0.value > $1.value }
                .map { k, v in
                    Completion(displayName: (k as NSString).lastPathComponent, fullPath: k,
                               breadcrumbText: "~" + k.dropFirst(5), isFrecent: true, matchRanges: [], score: v,
                               insertText: k + "/", lastVisit: UInt64(Date().timeIntervalSince1970))
                }
        }
        c.items.sort { $0.score > $1.score }
        // Tab's first press extends to the rows' common prefix (a unique folder gets "/").
        if pathLike, let first = c.items.first?.insertText {
            var common = first
            for i in c.items.dropFirst() {
                while !i.insertText.lowercased().hasPrefix(common.lowercased()) { common.removeLast() }
            }
            if c.items.count > 1, common.hasSuffix("/") { common.removeLast() }
            if common.count > text.count {
                c.commonPrefixExtension = TabExtension(replaceFrom: c.segmentStart, text: String(common.dropFirst(c.segmentStart)),
                                                       newInput: common)
            }
        }
        if let first = c.items.first, !text.isEmpty, first.insertText.hasPrefix(text) {
            c.ghostText = String(first.insertText.dropFirst(text.count))
        }
        return c
    }
    func copyPath(_ path: String, style: PathStyle) -> String {
        switch style {
        case .posix: path
        case .escaped: path.replacingOccurrences(of: " ", with: "\\ ")
        case .homeRelative: path.hasPrefix("/Mock") ? "~" + path.dropFirst(5) : path
        case .fileURL: URL(fileURLWithPath: path).absoluteString
        case .quoted: "'\(path)'"
        }
    }
    func importZoxide(dbPath: String?) throws -> (added: UInt32, merged: UInt32) {
        frecency["/Mock/Downloads"] = 3
        emit(.settingsChanged)
        return (42, 3)
    }
    func makeTabCycle() -> TabCycleAPI { MockTabCycle() }
    func deletePathComponent(_ input: String, cursor: Int) -> (text: String, cursor: Int) {
        let b = Array(input.utf8), c = min(cursor, b.count)
        var end = c
        if end > 0, b[end - 1] == UInt8(ascii: "/") { end -= 1 }
        var start = end
        while start > 0, b[start - 1] != UInt8(ascii: "/") { start -= 1 }
        if end == 0 { start = 0 }
        let text = String(decoding: b[..<start] + b[c...], as: UTF8.self)
        return (text, start)
    }
    func frecent(_ query: String, limit: UInt32) -> [FrecentDir] {
        frecency.sorted { $0.value > $1.value }.prefix(Int(limit)).map { FrecentDir(path: $0.key, score: $0.value, lastAccessed: 0) }
    }

    // MARK: Operations (fake: timers advance progress)

    private func startOp(_ kind: OpKind, _ title: String, files: UInt64, bytes: UInt64, into: String? = nil,
                         name: String? = nil, conflictWith: String? = nil) -> OpID {
        let id = nextOp; nextOp += 1
        var st = OpStatus(id: id, kind: kind, title: title, state: .running, filesTotal: max(files, 1), bytesTotal: bytes)
        if let dest = conflictWith {
            st.state = .waitingForDecision
            st.conflicts = [Conflict(op: id, id: 1, source: clipboard.first ?? dest, dest: dest, sourceKind: .file,
                                     destKind: .file, sourceSize: 1_200_000, destSize: 800_000,
                                     sourceMtimeNs: Int64(Date().timeIntervalSince1970 * 1e9), destMtimeNs: 0,
                                     canMerge: false)]
            emit(.opConflict(op: id, conflict: st.conflicts[0]))
        }
        ops.append(st)
        undoStack.append(title)
        emit(.opProgress([st]))
        emit(.undoChanged(undoState()))
        if conflictWith == nil { tick(id) }
        return id
    }

    private func tick(_ id: OpID) {
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.05) { [weak self] in
            guard let self, let i = self.ops.firstIndex(where: { $0.id == id }) else { return }
            var st = self.ops[i]
            guard st.state == .running else { return }
            st.bytesDone = min(st.bytesTotal, st.bytesDone + max(st.bytesTotal / 30, 1))
            st.filesDone = st.bytesTotal == 0 ? st.filesTotal : st.filesTotal * st.bytesDone / max(st.bytesTotal, 1)
            st.speedBps = Double(st.bytesTotal) / 1.5
            st.etaSecs = st.speedBps > 0 ? Double(st.bytesTotal - st.bytesDone) / st.speedBps : 0
            if st.bytesDone >= st.bytesTotal {
                st.state = .completed
                st.filesDone = st.filesTotal
                self.ops[i] = st
                self.emit(.opProgress([st]))
                self.emit(.opFinished(op: id, report: OpReport(id: id, kind: st.kind, state: .completed,
                                                                filesDone: st.filesTotal, bytesDone: st.bytesTotal,
                                                                elapsedMs: 1500)))
                return
            }
            self.ops[i] = st
            self.emit(.opProgress([st]))
            self.tick(id)
        }
    }

    private func setState(_ id: OpID, _ s: OpState) throws {
        guard let i = ops.firstIndex(where: { $0.id == id }) else { throw CoreError(.notFound, "no such operation") }
        ops[i].state = s
        emit(.opProgress([ops[i]]))
        if s == .cancelled {
            emit(.opFinished(op: id, report: OpReport(id: id, kind: ops[i].kind, state: .cancelled)))
        }
        if s == .running { tick(id) }
    }

    func enqueue(_ req: OpRequest) throws -> OpID { startOp(.copy, "Copy", files: 3, bytes: 90_000_000) }
    func opsList() -> [OpStatus] { ops }
    func opStatus(_ id: OpID) -> OpStatus? { ops.first { $0.id == id } }
    func pauseOp(_ id: OpID) throws { try setState(id, .paused) }
    func resumeOp(_ id: OpID) throws { try setState(id, .running) }
    func cancelOp(_ id: OpID) throws { try setState(id, .cancelled) }
    func reorderOp(_ id: OpID, to index: UInt32) throws {
        guard let i = ops.firstIndex(where: { $0.id == id }) else { return }
        let o = ops.remove(at: i)
        ops.insert(o, at: min(Int(index), ops.count))
    }
    func resolveConflict(op: OpID, conflict: ConflictID, decision: ConflictDecision, applyToAll: Bool) throws {
        guard let i = ops.firstIndex(where: { $0.id == op }) else { throw CoreError(.notFound, "no such operation") }
        ops[i].conflicts.removeAll { $0.id == conflict }
        try setState(op, .running)
    }
    func interruptedOps() throws -> [OpStatus] {
        [OpStatus(id: 999, kind: .copy, title: "Copy “Holiday” to Backup", state: .interrupted, filesDone: 120,
                  filesTotal: 300, bytesDone: 1_000_000_000, bytesTotal: 2_400_000_000)]
    }
    func clearFinishedOps() { ops.removeAll { $0.state.isFinished } }
    func undo() throws -> Outcome { Outcome(command: "edit.undo", message: undoStack.popLast().map { "Undid \($0)" }) }
    func redo() throws -> Outcome { Outcome(command: "edit.redo", message: "Nothing to redo") }
    func undoState() -> UndoState { UndoState(undo: undoStack.last.map { "Undo \($0)" }) }

    // MARK: Sidebar

    func volumes() -> [Volume] {
        [Volume(name: "Mock HD", mountPath: "/", totalBytes: 1_000_000_000_000, freeBytes: 241_500_000_000),
         Volume(name: "CARD", mountPath: "/Volumes/CARD", isRemovable: true, totalBytes: 128_000_000_000,
                freeBytes: 30_000_000_000)]
    }
    func favourites() -> [Favourite] {
        favs.map { var f = $0; f.missing = node(f.path)?.isDir != true; return f }
    }
    func addFavourite(_ path: String, index: UInt32?) throws {
        guard node(path)?.isDir == true else { throw CoreError(.notAvailable, "Only folders can be favourites") }
        guard !favs.contains(where: { $0.path == path }) else { return }
        favs.insert(Favourite(path: path, name: (path as NSString).lastPathComponent), at: min(Int(index ?? UInt32(favs.count)), favs.count))
        emit(.favouritesChanged)
    }
    func removeFavourite(_ path: String) throws { favs.removeAll { $0.path == path }; emit(.favouritesChanged) }
    func renameFavourite(_ path: String, name: String) throws {
        if let i = favs.firstIndex(where: { $0.path == path }) { favs[i].name = name; favs[i].renamed = true }
        emit(.favouritesChanged)
    }
    func moveFavourite(_ path: String, to index: UInt32) throws {
        guard let i = favs.firstIndex(where: { $0.path == path }) else { return }
        let f = favs.remove(at: i)
        favs.insert(f, at: min(Int(index), favs.count))
        emit(.favouritesChanged)
    }

    func volumesChanged() { emit(.volumesChanged(volumes())) }
    func appWillQuit() {}
}

// MARK: - Tab

final class MockTab: TabAPI, @unchecked Sendable {
    private unowned let engine: MockEngine
    let id: TabID
    private var loc: String
    private var backStack: [String] = []
    private var fwdStack: [String] = []
    private var viewState = ViewState()
    private var filterText = ""
    /// Visible children in order (for generated folders: indices into the generator).
    private var visible: [Node] = []
    private var dirNode: Node?
    private var selected = Set<UInt32>()
    private var cursor: UInt32?
    private var anchor: UInt32?
    private var version: UInt64 = 0
    private var scroll: ScrollAnchor?

    fileprivate init(engine: MockEngine, id: TabID, location: String) {
        self.engine = engine; self.id = id; self.loc = location
    }

    private var isGenerated: Bool { dirNode?.generated != nil }
    private var count: Int { isGenerated ? dirNode!.childCount : visible.count }

    private func nodeAt(_ i: Int) -> Node? {
        if isGenerated { return i < count ? dirNode!.generated(i) : nil }
        return visible.indices.contains(i) ? visible[i] : nil
    }

    fileprivate func load() {
        dirNode = engine.node(loc)
        selected = []; cursor = nil; anchor = nil
        if let d = dirNode, d.generated == nil {
            var kids = d.kids.filter { (viewState.showHidden || !$0.name.hasPrefix(".")) &&
                (filterText.isEmpty || $0.name.localizedCaseInsensitiveContains(filterText)) }
            kids.sort { a, b in
                if viewState.foldersFirst, a.isDir && !a.isPackage, !(b.isDir && !b.isPackage) { return true }
                if viewState.foldersFirst, b.isDir && !b.isPackage, !(a.isDir && !a.isPackage) { return false }
                let r: Bool = switch viewState.sort {
                case .size: a.size < b.size
                case .modified, .created: a.mtimeNs < b.mtimeNs
                default: a.name.localizedStandardCompare(b.name) == .orderedAscending
                }
                return viewState.ascending ? r : !r
            }
            visible = kids
        } else {
            visible = []
        }
        version += 1
        engine.emit(.listingChanged(tab: id, version: version, kind: dirNode == nil ? .error : .sorted))
        engine.emit(.tabsChanged)
    }

    fileprivate func navigate(_ path: String, push: Bool = true) {
        if push, path != loc { backStack.append(loc); fwdStack = [] }
        loc = path
        engine.frecency[path, default: 0] += 1
        load()
    }

    private func changed() {
        engine.emit(.selectionChanged(tab: id, count: UInt32(selected.count), cursor: cursor))
    }

    func location() -> String { loc }
    func state() -> TabState {
        TabState(id: id, location: loc, back: backStack, forward: fwdStack, view: viewState, filter: filterText,
                 selected: selectedPaths().map { ($0 as NSString).lastPathComponent },
                 cursor: cursor.flatMap { nodeAt(Int($0))?.name }, scroll: scroll)
    }
    func listingInfo() -> ListingInfo {
        ListingInfo(version: version, dir: loc, state: dirNode == nil ? .error : .complete,
                    error: dirNode == nil ? "The folder doesn't exist" : nil,
                    count: UInt32(count), totalCount: UInt32(dirNode?.childCount ?? 0))
    }
    func rows(start: UInt32, count n: UInt32) -> [Row] {
        let end = min(Int(start) + Int(n), count)
        guard Int(start) < end else { return [] }
        let df = DateFormatter(); df.dateStyle = .medium; df.timeStyle = .short
        let bf = ByteCountFormatter()
        let cut = Set(engine.clipboard)
        return (Int(start)..<end).compactMap { i in
            guard let k = nodeAt(i) else { return nil }
            let ext = (k.name as NSString).pathExtension.lowercased()
            let kind: RowKind = k.isPackage ? .package : k.isDir ? .folder : .file
            var r = Row(index: UInt32(i), name: k.name, kind: kind,
                       iconKey: k.isPackage ? "package.\(ext)" : k.isDir ? "folder" : ext.isEmpty ? "file" : "file.\(ext)",
                       kindText: k.isPackage ? "Application" : k.isDir ? "Folder" : ext.isEmpty ? "Document" : "\(ext.uppercased()) file",
                       size: k.size, sizeText: k.isDir ? "--" : bf.string(fromByteCount: Int64(k.size)),
                       modifiedNs: k.mtimeNs,
                       modifiedText: df.string(from: Date(timeIntervalSince1970: Double(k.mtimeNs) / 1e9)),
                       isHidden: k.name.hasPrefix("."), isDataless: k.isDataless, isPackage: k.isPackage,
                       isSelected: selected.contains(UInt32(i)), isCursor: cursor == UInt32(i),
                       isCutPending: cut.contains(loc + "/" + k.name))
            // ui-list additions
            r.createdNs = k.mtimeNs - 86_400_000_000_000
            r.createdText = df.string(from: Date(timeIntervalSince1970: Double(r.createdNs) / 1e9))
            r.extText = k.isDir && !k.isPackage ? "" : (k.name as NSString).pathExtension
            return r
        }
    }
    func indexOf(name: String) -> UInt32? {
        if isGenerated { return dirNode!.child(name) != nil ? UInt32(name.dropFirst(5).prefix(6)) : nil }
        return visible.firstIndex { $0.name == name }.map(UInt32.init)
    }
    func pathAt(_ row: UInt32) -> String? { nodeAt(Int(row)).map { loc + "/" + $0.name } }
    func go(_ to: String) throws -> Outcome {
        let p = engine.parseLocation(to, cwd: loc)
        switch p.location {
        case .dir(let d):
            // Going to an ancestor selects the child we came from (as the core does).
            let from = loc
            navigate(d)
            let base = d == "/" ? "/" : d + "/"
            if from.hasPrefix(base), let child = from.dropFirst(base.count).split(separator: "/").first,
               let i = indexOf(name: String(child)) { setSelection(.only(i)) }
        case .file(let d, let n):
            navigate(d)
            if let i = indexOf(name: n) { setSelection(.only(i)) }
        case .missing(let path, let near): return Outcome(tab: id, shell: .offerCreateFolder(path: path, nearestExisting: near))
        case .remote: throw CoreError(.notAvailable, "Remote locations come later")
        }
        return Outcome(tab: id)
    }
    func back() throws -> Outcome {
        guard let p = backStack.popLast() else { throw CoreError(.notAvailable, "nothing to go back to") }
        fwdStack.insert(loc, at: 0); navigate(p, push: false); return Outcome(tab: id)
    }
    func forward() throws -> Outcome {
        guard !fwdStack.isEmpty else { throw CoreError(.notAvailable, "nothing to go forward to") }
        let p = fwdStack.removeFirst(); backStack.append(loc); navigate(p, push: false); return Outcome(tab: id)
    }
    func up() throws -> Outcome {
        guard loc != "/Mock" else { throw CoreError(.notAvailable, "already at the top") }
        let child = (loc as NSString).lastPathComponent
        navigate((loc as NSString).deletingLastPathComponent)
        if let i = indexOf(name: child) { setSelection(.only(i)) }
        return Outcome(tab: id)
    }
    var canGoBack: Bool { !backStack.isEmpty }
    var canGoForward: Bool { !fwdStack.isEmpty }
    func refresh() { load() }
    func view() -> ViewState { viewState }
    func setView(_ v: ViewState) { viewState = v; load() }
    func filter() -> String { filterText }
    func setFilter(_ text: String) { filterText = text; load() }
    func selection() -> Selection {
        var s = Selection()
        s.rows = selected.sorted(); s.names = s.rows.compactMap { nodeAt(Int($0))?.name }
        s.cursor = cursor; s.anchor = anchor; s.version = version
        return s
    }
    func selectedPaths() -> [String] { selected.sorted().compactMap { pathAt($0) } }
    func setSelection(_ change: SelectionChange) {
        let n = UInt32(count)
        switch change {
        case .only(let r): selected = [r]; cursor = r; anchor = r
        case .toggle(let r): if selected.contains(r) { selected.remove(r) } else { selected.insert(r) }; cursor = r; anchor = r
        case .extendTo(let r):
            let a = anchor ?? r
            selected = Set(min(a, r)...max(a, r)); cursor = r
        case .all: selected = n == 0 ? [] : Set(0..<n)
        case .none: selected = []
        case .invert: selected = Set((0..<n).filter { !selected.contains($0) })
        case .rows(let rs): selected = Set(rs); cursor = rs.first
        case .names(let ns): selected = Set(ns.compactMap { indexOf(name: $0) }); cursor = selected.min()
        case .move(let m, let extend):
            guard n > 0 else { return }
            let c = cursor ?? 0
            let next: UInt32 = switch m {
            case .up: c == 0 ? 0 : c - 1
            case .down: min(n - 1, cursor == nil ? 0 : c + 1)
            case .pageUp: c > 20 ? c - 20 : 0
            case .pageDown: min(n - 1, c + 20)
            case .home: 0
            case .end: n - 1
            }
            if extend { let a = anchor ?? c; selected = Set(min(a, next)...max(a, next)) } else { selected = [next]; anchor = next }
            cursor = next
        }
        changed()
    }
    func selectPattern(_ pattern: String, select: Bool) {
        let rs = (0..<UInt32(min(count, 10_000))).filter { nodeAt(Int($0))?.name.localizedCaseInsensitiveContains(pattern) == true }
        if select { selected.formUnion(rs) } else { selected.subtract(rs) }
        changed()
    }
    func typeSelect(_ text: String) -> UInt32? {
        let t = text.lowercased()
        let hit: UInt32? = isGenerated ? nil
            : visible.firstIndex { $0.name.lowercased().hasPrefix(t) }.map(UInt32.init)
        if let h = hit { setSelection(.only(h)) }
        return hit
    }
    func setVisibleRange(start: UInt32, count: UInt32) {}
    func setScrollAnchor(_ a: ScrollAnchor?) { scroll = a }
    func scrollAnchor() -> ScrollAnchor? { scroll }
    func breadcrumbs() -> [Crumb] { engine.breadcrumbs(path: loc) }
    func complete(_ input: String, cursor: UInt32) -> Completions { engine.complete(input, cursor: cursor, cwd: loc) }
}

/// Imitates the core's Tab cycle (extend, then select row 1, then cycle).
final class MockTabCycle: TabCycleAPI {
    private var rows: [String] = []
    private var index: Int?
    func press(_ c: Completions, backwards: Bool) -> TabOutcome {
        if let cur = index {
            guard !rows.isEmpty else { return .nothing }
            let n = rows.count, next = backwards ? (cur + n - 1) % n : (cur + 1) % n
            index = next
            return .select(index: next, text: rows[next])
        }
        if !backwards, let e = c.commonPrefixExtension { return .extend(newInput: e.newInput) }
        guard !c.items.isEmpty else { return .nothing }
        rows = c.items.map(\.insertText)
        let first = backwards ? rows.count - 1 : 0
        index = first
        return .select(index: first, text: rows[first])
    }
    func reset() { rows = []; index = nil }
    var isCycling: Bool { index != nil }
}

// MARK: - ui-list additions

/// A tab-less listing: a private MockTab that isn't in the workspace.
final class MockListing: ListingAPI, @unchecked Sendable {
    fileprivate let tab: MockTab
    var id: TabID { tab.id }
    fileprivate init(tab: MockTab) { self.tab = tab }
    func dir() -> String { tab.location() }
    func listingInfo() -> ListingInfo { tab.listingInfo() }
    func rows(start: UInt32, count: UInt32) -> [Row] { tab.rows(start: start, count: count) }
    func indexOf(name: String) -> UInt32? { tab.indexOf(name: name) }
    func pathAt(_ row: UInt32) -> String? { tab.pathAt(row) }
    func selection() -> Selection { tab.selection() }
    func setSelection(_ change: SelectionChange) { tab.setSelection(change) }
    func selectedPaths() -> [String] { tab.selectedPaths() }
    func setVisibleRange(start: UInt32, count: UInt32) {}
    func refresh() { tab.refresh() }
    func menuTarget(owner: TabID, row: UInt32?) -> MenuTarget {
        guard let row, row < tab.listingInfo().count else { tab.setSelection(.none); return .emptySpace(tab: owner) }
        if !tab.selection().rows.contains(row) { tab.setSelection(.only(row)) }
        let paths = tab.selectedPaths()
        if paths.count > 1 { return .multi(tab: owner, paths: paths) }
        let r = tab.rows(start: row, count: 1).first
        return r?.kind == .package ? .package(tab: owner, path: paths[0])
            : r?.kind == .folder ? .folder(tab: owner, path: paths[0]) : .file(tab: owner, path: paths[0])
    }
}

extension MockEngine: ListEngineAPI {
    func openListing(path: String, showHidden: Bool) -> ListingAPI {
        let t = MockTab(engine: self, id: newID(), location: path)
        var v = ViewState(); v.showHidden = showHidden
        t.setView(v)
        return MockListing(tab: t)
    }
    func columnSelect(tab: TabID, dir: String, name: String?) throws -> Outcome {
        guard let t = tabs[tab] else { throw CoreError(.notFound, "tab \(tab)") }
        if t.location() != dir { t.navigate(dir) }
        if let name, let i = t.indexOf(name: name) { t.setSelection(.only(i)) } else { t.setSelection(.none) }
        return Outcome(tab: tab)
    }
    func dropEffect(sources: [String], dest: String, mods: Mods) -> DropEffect {
        guard !sources.isEmpty, let d = node(dest), d.isDir, !d.isPackage else { return .none }
        if sources.contains(where: { dest == $0 || dest.hasPrefix($0 + "/") }) { return .none }
        if mods.alt { return .copy }
        if sources.allSatisfy({ ($0 as NSString).deletingLastPathComponent == dest }) { return .none }
        if mods.cmd { return .move }
        return sources.allSatisfy { $0.hasPrefix("/Mock/") } ? .move : .copy
    }
    func dropItems(sources: [String], dest: String, mods: Mods) throws -> Outcome {
        try dropFiles(sources: sources, destDir: dest, mods: mods)
    }
    func sortByColumn(tab: TabID, column: ColumnID) throws -> Outcome {
        guard let t = tabs[tab] else { throw CoreError(.notFound, "tab \(tab)") }
        var v = t.view()
        if v.sort == column.sortBy { v.ascending.toggle() } else {
            v.sort = column.sortBy
            v.ascending = ![.size, .modified, .created].contains(column)
        }
        t.setView(v)
        return Outcome(tab: tab)
    }
    /// ←/→ in column view (the core's rule, imitated).
    fileprivate func columnKey(_ chord: KeyChord, context: InputContext) -> Outcome? {
        guard context.focus == .list, chord.mods.isEmpty, chord.key == .left || chord.key == .right,
              let t = (context.tab.flatMap { tabs[$0] } ?? activeTab() as? MockTab), t.view().mode == .columns
        else { return nil }
        if chord.key == .left {
            _ = try? t.up()
        } else if let c = t.selection().cursor, let r = t.rows(start: c, count: 1).first, r.kind == .folder {
            t.navigate(t.location() + "/" + r.name)
        }
        return Outcome(tab: t.id)
    }
}

// Workspaces.swift — the shell side of workspaces (F-002): a switcher in the
// toolbar, the Workspaces menu's list, the New Workspace and Manage
// Workspaces sheets.
//
// The core owns every workspace and what it holds (windows, panes, tabs,
// history, view options, split, sidebar width). Switching, creating,
// renaming, reordering, duplicating and deleting are all core calls or
// registered commands (`workspace.*`, F-006); this file only draws them and
// shows the core's errors. Deleting goes through the core's question
// (`confirm`), and touches only the saved layout, never files (F-007).

import AppKit
import SwiftUI

// MARK: - Menu items

/// A menu item that switches to the workspace at `index`, through the
/// `workspace.switch` command, so it is the same path as ⌃⌘1….
@MainActor
final class WorkspaceMenuItem: NSMenuItem {
    let index: UInt32
    private weak var model: AppModel?
    private let window: WindowID?

    init(_ w: WorkspaceSummary, index: Int, model: AppModel, window: WindowID?, keys: Bool) {
        self.index = UInt32(index)
        self.model = model
        self.window = window
        super.init(title: w.name, action: #selector(fire), keyEquivalent: "")
        target = self
        image = NSImage(systemSymbolName: w.icon, accessibilityDescription: nil)
            ?? NSImage(systemSymbolName: "square.grid.2x2", accessibilityDescription: nil)
        state = w.active ? .on : .off
        toolTip = "\(w.tabs) tab\(w.tabs == 1 ? "" : "s")" + (w.shortcut.map { " · \($0)" } ?? "")
        if keys, let s = w.shortcut, let (key, mods) = MainMenu.keyEquivalent(s) {
            keyEquivalent = key
            keyEquivalentModifierMask = mods
        }
    }

    required init(coder: NSCoder) { fatalError() }

    @objc private func fire() {
        // A key press reached the core first (KeyRouter); the menu only shows the key.
        if NSApp.currentEvent?.type == .keyDown { return }
        guard let model else { return }
        let (ctx, wid) = KeyRouter.context(in: NSApp.keyWindow)
        model.perform(Invocation(id: "workspace.switch", arg: .index(index), tab: ctx.tab), via: .menu,
                      window: window ?? wid)
    }
}

@MainActor
enum WorkspaceMenus {
    /// Tag on the items the Workspaces menu rebuilds each time it opens.
    static let dynamicTag = 0x5753

    /// Adds one item per workspace (the active one ticked).
    static func addList(to menu: NSMenu, model: AppModel, window: WindowID?, keys: Bool) {
        for (i, w) in model.workspaces.enumerated() {
            let it = WorkspaceMenuItem(w, index: i, model: model, window: window, keys: keys)
            it.tag = dynamicTag
            menu.addItem(it)
        }
    }

    /// The switcher's menu: the workspaces, then New and Manage.
    static func switcherMenu(model: AppModel, window: WindowID?) -> NSMenu {
        let m = NSMenu()
        addList(to: m, model: model, window: window, keys: false)
        m.addItem(.separator())
        for (id, title) in [("workspace.save", "New Workspace…"), ("workspace.manage", "Manage Workspaces…")] {
            m.addItem(ActionItem(title: title) { [weak model] in
                model?.perform(Invocation(id: id), via: .mouse, window: window)
            })
        }
        return m
    }
}

/// Keeps the menu bar's Workspaces menu listing the workspaces (MainMenu
/// builds its commands from the registry; this adds the list under them).
@MainActor
final class WorkspacesMenuDelegate: NSObject, NSMenuDelegate {
    static let shared = WorkspacesMenuDelegate()
    weak var model: AppModel?

    func menuNeedsUpdate(_ menu: NSMenu) {
        for it in menu.items where it.tag == WorkspaceMenus.dynamicTag { menu.removeItem(it) }
        guard let model, !model.workspaces.isEmpty else { return }
        let sep = NSMenuItem.separator()
        sep.tag = WorkspaceMenus.dynamicTag
        menu.addItem(sep)
        WorkspaceMenus.addList(to: menu, model: model, window: nil, keys: true)
    }
}

// MARK: - Toolbar switcher

/// The toolbar's workspace switcher: the active workspace's icon and name;
/// clicking lists every workspace, then New and Manage (ForkLift-style).
@MainActor
final class WorkspaceSwitcher: NSPopUpButton {
    private unowned let model: AppModel
    private let coreWindow: WindowID

    init(model: AppModel, window: WindowID) {
        self.model = model
        coreWindow = window
        super.init(frame: NSRect(x: 0, y: 0, width: 150, height: 28), pullsDown: true)
        bezelStyle = .toolbar
        imagePosition = .imageLeading
        (cell as? NSPopUpButtonCell)?.arrowPosition = .arrowAtBottom
        setAccessibilityLabel("Workspace")
        refresh()
    }

    required init?(coder: NSCoder) { fatalError() }

    /// The menu that opens on a click: the workspaces, then New and Manage.
    /// It never holds the pull-down's title item, because a toolbar
    /// pull-down draws that item in its open menu (even when hidden) and the
    /// active workspace showed twice.
    private var itemsMenu = NSMenu()

    /// Rebuilds the label and the menu from `model.workspaces`.
    func refresh() {
        let active = model.workspaces.first { $0.active }
        itemsMenu = WorkspaceMenus.switcherMenu(model: model, window: coreWindow)
        // The button's own menu holds only its title item, which labels it.
        let title = NSMenuItem(title: active?.name ?? "Workspace", action: nil, keyEquivalent: "")
        title.image = NSImage(systemSymbolName: active?.icon ?? "square.grid.2x2", accessibilityDescription: nil)
        let label = NSMenu()
        label.addItem(title)
        menu = label
        toolTip = "Workspace: \(active?.name ?? "—"). Each workspace keeps its own tabs and layout."
        sizeToFit()
        frame.size.width = min(max(frame.width, 96), 220)
    }

    /// Opens the workspace list under the button.
    func openList() {
        refresh()  // Tab counts may have changed since the last refresh.
        highlight(true)
        defer { highlight(false) }
        itemsMenu.popUp(positioning: nil, at: NSPoint(x: 0, y: bounds.maxY + 4), in: self)
    }

    override func mouseDown(with event: NSEvent) { openList() }
    override func performClick(_ sender: Any?) { openList() }
    override func accessibilityPerformPress() -> Bool { openList(); return true }

    /// Debug: the menu that opens on a click, as lines.
    var debugLines: [String] {
        itemsMenu.items.filter { !$0.isHidden }.map { $0.isSeparatorItem ? "---" : "\($0.state == .on ? "[x]" : "[ ]") \($0.title)" }
    }
}

// MARK: - Sheets

/// The New Workspace sheet's state.
@MainActor
@Observable
final class NewWorkspaceForm {
    var name = ""
    var icon = "square.grid.2x2"
    var copyCurrent = false
    var error: String?
    let icons: [String]
    init(icons: [String]) {
        self.icons = icons
        if let first = icons.first(where: { $0 == "square.grid.2x2" }) ?? icons.first { icon = first }
    }
}

struct IconGrid: View {
    let icons: [String]
    @Binding var selection: String

    var body: some View {
        LazyVGrid(columns: Array(repeating: GridItem(.fixed(30), spacing: 6), count: 7), spacing: 6) {
            ForEach(icons, id: \.self) { name in
                Button { selection = name } label: {
                    Image(systemName: name)
                        .font(.system(size: 14))
                        .frame(width: 30, height: 26)
                        .background(RoundedRectangle(cornerRadius: 6)
                            .fill(selection == name ? Color.accentColor.opacity(0.25) : Color.clear))
                        .overlay(RoundedRectangle(cornerRadius: 6)
                            .strokeBorder(selection == name ? Color.accentColor : Color.clear, lineWidth: 1.5))
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .help(name)
                .accessibilityLabel(name)
            }
        }
    }
}

struct NewWorkspaceView: View {
    @Bindable var form: NewWorkspaceForm
    let current: String
    let create: () -> Void
    let cancel: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("New Workspace").font(.headline)
            Text("A workspace keeps its own windows, tabs and layout. Switching saves the one you leave.")
                .font(.callout).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            Form {
                TextField("Name:", text: $form.name, prompt: Text("Photo work"))
                    .onSubmit(create)
                LabeledContent("Icon:") {
                    IconGrid(icons: form.icons, selection: $form.icon)
                        .alignmentGuide(.firstTextBaseline) { $0[.top] + 18 }
                }
                Picker("Start with:", selection: $form.copyCurrent) {
                    Text("One tab at Home").tag(false)
                    Text("A copy of “\(current)”").tag(true)
                }
                .pickerStyle(.radioGroup)
            }
            if let e = form.error {
                Label(e, systemImage: "exclamationmark.triangle.fill")
                    .foregroundStyle(.orange).font(.callout)
            }
            HStack {
                Spacer()
                Button("Cancel", action: cancel).keyboardShortcut(.cancelAction)
                Button("Create", action: create).keyboardShortcut(.defaultAction)
                    .disabled(form.name.trimmingCharacters(in: .whitespaces).isEmpty)
            }
        }
        .padding(20)
        .frame(width: 400)
    }
}

/// One row of the Manage sheet.
struct ManageRow: View {
    let w: WorkspaceSummary
    let icons: [String]
    let rename: (String) -> String?
    let setIcon: (String) -> Void
    @State private var name = ""
    @State private var error: String?

    var body: some View {
        HStack(spacing: 10) {
            Menu {
                ForEach(icons, id: \.self) { i in
                    Button { setIcon(i) } label: { Label(i, systemImage: i) }
                }
            } label: {
                Image(systemName: w.icon).frame(width: 22)
            }
            .menuStyle(.borderlessButton)
            .menuIndicator(.hidden)
            .fixedSize()
            .help("Change the icon")
            VStack(alignment: .leading, spacing: 2) {
                TextField("Name", text: $name)
                    .textFieldStyle(.plain)
                    .onSubmit { commit() }
                if let error {
                    Text(error).font(.caption).foregroundStyle(.orange)
                } else {
                    Text("\(w.tabs) tab\(w.tabs == 1 ? "" : "s")" + (w.windows > 1 ? " in \(w.windows) windows" : ""))
                        .font(.caption).foregroundStyle(.secondary)
                }
            }
            Spacer()
            if w.active { Text("Current").font(.caption).foregroundStyle(.secondary) }
            if let s = w.shortcut { Text(s).font(.caption).monospacedDigit().foregroundStyle(.secondary) }
        }
        .padding(.vertical, 3)
        .onAppear { name = w.name }
        .onChange(of: w.name) { _, n in name = n }
    }

    private func commit() {
        error = rename(name)
        if error != nil { name = w.name }
    }
}

struct ManageWorkspacesView: View {
    let model: AppModel
    let done: () -> Void
    @State private var selection: WorkspaceID?
    @State private var message: String?

    private var list: [WorkspaceSummary] { model.workspaces }
    private var selected: WorkspaceSummary? { list.first { $0.id == selection } }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Workspaces").font(.headline)
            Text("Each workspace keeps its own windows, tabs and layout. Drag to reorder; ⌃⌘1…⌃⌘9 follow this order. Favourites are shared by all workspaces.")
                .font(.callout).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            List(selection: $selection) {
                ForEach(list) { w in
                    ManageRow(w: w, icons: model.engine.workspaceIcons(),
                              rename: { rename(w, $0) },
                              setIcon: { icon in run { try model.engine.setWorkspaceIcon(w.id, icon: icon) } })
                        .tag(w.id)
                }
                .onMove { from, to in
                    guard let f = from.first else { return }
                    let id = list[f].id
                    run { try model.engine.moveWorkspace(id, to: UInt32(to > f ? to - 1 : to)) }
                }
            }
            .listStyle(.bordered(alternatesRowBackgrounds: false))
            .frame(minHeight: 200)
            if let message {
                Label(message, systemImage: "exclamationmark.triangle.fill").foregroundStyle(.orange).font(.callout)
            }
            HStack {
                Button { model.perform(Invocation(id: "workspace.save"), via: .mouse) } label: { Image(systemName: "plus") }
                    .help("New Workspace…")
                Button("Duplicate") {
                    guard let s = selected else { return }
                    run { _ = try model.engine.duplicateWorkspace(s.id) }
                }
                .disabled(selected == nil)
                Button("Delete…") {
                    guard let s = selected else { return }
                    run { model.handle(try model.engine.askDeleteWorkspace(s.id), in: model.keyWindowID) }
                }
                .disabled(selected == nil || list.count < 2)
                .help("Forgets the workspace's tabs and layout. No files are touched.")
                Button("Switch To") {
                    guard let s = selected else { return }
                    run { _ = try model.engine.switchWorkspace(s.id) }
                }
                .disabled(selected == nil || selected?.active == true)
                Spacer()
                Button("Done", action: done).keyboardShortcut(.defaultAction)
            }
        }
        .padding(20)
        .frame(width: 460, height: 400)
        .onAppear { selection = list.first { $0.active }?.id }
    }

    private func rename(_ w: WorkspaceSummary, _ name: String) -> String? {
        guard name != w.name else { return nil }
        do { try model.engine.renameWorkspace(w.id, name: name); return nil } catch { return error.localizedDescription }
    }

    private func run(_ body: () throws -> Void) {
        do { try body(); message = nil } catch { message = error.localizedDescription }
    }
}

// MARK: - Presenting

@MainActor
enum WorkspaceSheets {
    private static var sheet: NSWindow?
    private static weak var host: NSWindow?
    private(set) static var newForm: NewWorkspaceForm?

    /// Handles the core's `askWorkspaceName` (New Workspace…) and
    /// `manageWorkspaces` requests.
    static func install(_ model: AppModel) {
        WorkspacesMenuDelegate.shared.model = model
        model.registerShellHandler(for: "askWorkspaceName") { _, ctx in
            showNew(model: model, window: ctx.window)
            return true
        }
        model.registerShellHandler(for: "manageWorkspaces") { _, ctx in
            showManage(model: model, window: ctx.window)
            return true
        }
        #if DEBUG
        WorkspaceDebugSteps.install()
        #endif
    }

    private static func hostWindow(_ id: WindowID?) -> NSWindow? {
        let mains = NSApp.windows.filter { $0.windowController is CoreWindowIdentifying && $0.isVisible }
        if let id, let w = mains.first(where: { ($0.windowController as? CoreWindowIdentifying)?.coreWindowID == id }) {
            return w
        }
        return NSApp.keyWindow.flatMap { k in mains.first { $0 === k || $0 === k.sheetParent } } ?? mains.first
    }

    static func showNew(model: AppModel, window: WindowID?) {
        // From the Manage sheet: close it first, the new workspace is what comes next.
        let fromManage = sheet != nil && newForm == nil
        if fromManage { close() }
        guard sheet == nil, let host = hostWindow(window) else { return }
        let form = NewWorkspaceForm(icons: model.engine.workspaceIcons())
        newForm = form
        let current = model.workspaces.first { $0.active }?.name ?? "this workspace"
        let view = NewWorkspaceView(form: form, current: current, create: { create(model) }, cancel: { close() })
        present(NSHostingController(rootView: view), on: host)
    }

    /// The sheet's Create button.
    static func create(_ model: AppModel) {
        guard let form = newForm else { return }
        do {
            _ = try model.engine.createWorkspace(name: form.name, icon: form.icon, copyCurrent: form.copyCurrent)
            close()
        } catch {
            form.error = error.localizedDescription
        }
    }

    static func showManage(model: AppModel, window: WindowID?) {
        guard sheet == nil, let host = hostWindow(window) else { return }
        let view = ManageWorkspacesView(model: model, done: { close() })
        present(NSHostingController(rootView: view), on: host)
    }

    private static func present(_ vc: NSViewController, on host: NSWindow) {
        let w = NSWindow(contentViewController: vc)
        w.styleMask = [.titled]
        sheet = w
        self.host = host
        host.beginSheet(w) { _ in }
    }

    static func close() {
        if let s = sheet { host?.endSheet(s) ?? s.close() }
        sheet = nil
        newForm = nil
    }

    static var isShowing: Bool { sheet != nil }
}

// MARK: - Debug steps

#if DEBUG
/// Debug-automation steps:
///
///   workspaces                    WORKSPACE lines (position, name, icon, tabs, active, key)
///   wsmenu                        WSMENU lines: the key window's switcher menu
///   wsform:<name>|<icon>|<empty|copy>   fill the open New Workspace sheet
///   wscreate                      press its Create button
///   wsclose                       close the open workspace sheet
///   wsmove:<from>,<to>            reorder, as a drag in the Manage sheet would
///   wsicon:<index>,<symbol>       change a workspace's icon
///   wsdelete:<index>              ask to delete (answer with alert:ok/cancel or the CONFIRM flow)
@MainActor
enum WorkspaceDebugSteps {
    private static var installed = false

    static func install() {
        guard !installed else { return }
        installed = true
        DebugAutomation.register("workspaces") { _, m in
            for w in m.workspaces {
                print("WORKSPACE \(w.position) \(w.name) icon=\(w.icon) tabs=\(w.tabs) windows=\(w.windows)"
                      + (w.active ? " active" : "") + (w.shortcut.map { " key=\($0)" } ?? ""))
            }
            for w in m.workspace.windows {
                print("WSLAYOUT window panes=\(w.panes.count) tabs=\(w.panes.map { String($0.tabs.count) }.joined(separator: "+")) "
                      + "orientation=\(w.orientation) ratio=\(String(format: "%.2f", w.splitRatio)) "
                      + "sidebar=\(Int(w.sidebarWidth))\(w.sidebarHidden ? " hidden" : "") "
                      + "locations=\(w.panes.flatMap { $0.tabs.map { ($0.location as NSString).lastPathComponent } }.joined(separator: ","))")
            }
        }
        DebugAutomation.register("wsmenu") { _, _ in
            let sw = NSApp.windows.compactMap { $0.toolbar?.items.compactMap { $0.view as? WorkspaceSwitcher }.first }.first
            print("WSMENU title=\(sw?.titleOfSelectedItem ?? sw?.itemTitle(at: 0) ?? "none")")
            for l in sw?.debugLines ?? [] { print("WSMENU \(l)") }
            if let ws = NSApp.mainMenu?.items.first(where: { $0.title == "Workspaces" })?.submenu {
                WorkspacesMenuDelegate.shared.menuNeedsUpdate(ws)
                for it in ws.items {
                    let key = it.keyEquivalent.isEmpty ? "" : " key=\(it.keyEquivalentModifierMask.contains(.control) ? "⌃" : "")\(it.keyEquivalentModifierMask.contains(.command) ? "⌘" : "")\(it.keyEquivalent)"
                    print("MAINMENU Workspaces: " + (it.isSeparatorItem ? "---" : "\(it.state == .on ? "[x] " : "")\(it.title)\(key)"))
                }
            }
        }
        DebugAutomation.register("wsform") { arg, _ in
            guard let f = WorkspaceSheets.newForm else { print("WSFORM none"); return }
            let p = arg.split(separator: "|", omittingEmptySubsequences: false).map(String.init)
            f.name = p.first ?? ""
            if p.count > 1, !p[1].isEmpty { f.icon = p[1] }
            if p.count > 2 { f.copyCurrent = p[2] == "copy" }
            print("WSFORM name=\(f.name) icon=\(f.icon) copy=\(f.copyCurrent)")
        }
        DebugAutomation.register("wscreate") { _, m in
            WorkspaceSheets.create(m)
            print("WSCREATE " + (WorkspaceSheets.newForm?.error.map { "error: \($0)" } ?? "ok"))
        }
        DebugAutomation.register("wsclose") { _, _ in WorkspaceSheets.close() }
        DebugAutomation.register("wsmove") { arg, m in
            let p = arg.split(separator: ",").compactMap { Int($0) }
            guard p.count == 2, m.workspaces.indices.contains(p[0]) else { return }
            do { try m.engine.moveWorkspace(m.workspaces[p[0]].id, to: UInt32(p[1])) } catch { print("DEBUG \(error)") }
        }
        DebugAutomation.register("wsicon") { arg, m in
            let p = arg.split(separator: ",").map(String.init)
            guard p.count == 2, let i = Int(p[0]), m.workspaces.indices.contains(i) else { return }
            do { try m.engine.setWorkspaceIcon(m.workspaces[i].id, icon: p[1]) } catch { print("DEBUG \(error)") }
        }
        DebugAutomation.register("wsdelete") { arg, m in
            guard let i = Int(arg), m.workspaces.indices.contains(i) else { return }
            do { m.handle(try m.engine.askDeleteWorkspace(m.workspaces[i].id), in: m.keyWindowID) } catch {
                print("WSDELETE error: \(error.localizedDescription)")
            }
        }
    }
}
#endif

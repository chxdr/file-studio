// SettingsMenus.swift — the Menus tab: customise each context menu (F-022).
// Pick what was right-clicked, then add, remove and drag items; the core
// stores the changes as per-menu overrides and says what the menu holds.

import SwiftUI

extension MenuKind {
    var settingsTitle: String {
        switch self {
        case .file: "File"
        case .folder: "Folder"
        case .package: "App or package"
        case .multi: "Several items"
        case .emptySpace: "Empty space in a folder"
        case .crumb: "Path-bar segment"
        case .sidebarItem: "Sidebar item"
        case .tab: "Tab"
        }
    }
}

struct SettingsMenusPane: View {
    let model: AppModel
    @State private var kind: MenuKind = .file
    @State private var entries: [MenuEntry] = []
    @State private var adding = false
    @State private var error: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Picker("Menu for", selection: $kind) {
                    ForEach(MenuKind.allCases, id: \.self) { Text($0.settingsTitle).tag($0) }
                }
                .frame(width: 320)
                Spacer()
                Button("Add Item…") { adding = true }
                    .popover(isPresented: $adding, arrowEdge: .bottom) {
                        MenuChoicePicker(choices: model.engine.menuChoices(kind)) { add($0) }
                    }
            }
            Text("Drag to reorder. Right-click a folder, file or empty space to see the result.")
                .font(.caption).foregroundStyle(.secondary)
            List {
                ForEach(Array(entries.enumerated()), id: \.offset) { _, e in
                    HStack {
                        Image(systemName: "line.3.horizontal").foregroundStyle(.tertiary)
                        if e.entry == "---" {
                            Text("Separator").foregroundStyle(.secondary).italic()
                            Spacer()
                            VStack { Divider() }.frame(width: 120)
                        } else {
                            Text(e.title)
                            Spacer()
                        }
                        Button { remove(e) } label: { Image(systemName: "minus.circle") }
                            .buttonStyle(.borderless).help("Remove from this menu")
                    }
                }
                .onMove(perform: move)
            }
            .listStyle(.inset)
            if let error { Label(error, systemImage: "exclamationmark.triangle.fill").font(.callout).foregroundStyle(.red) }
            HStack {
                Button("Reset This Menu") { run { try model.engine.resetMenu(kind) } }
                Spacer()
                Button("Reset All Menus") { run { try model.engine.resetMenus() } }
            }
        }
        .padding(16)
        .onAppear(perform: reload)
        .onChange(of: kind) { reload() }
        .onChange(of: model.settings) { reload() }
    }

    private func reload() { entries = model.engine.menuEntries(kind) }

    private func add(_ e: MenuEntry) {
        adding = false
        run { try model.engine.menuAdd(kind, entry: e.entry, after: entries.last?.entry) }
    }

    private func remove(_ e: MenuEntry) { run { try model.engine.menuRemove(kind, entry: e.entry) } }

    private func move(_ from: IndexSet, _ to: Int) {
        guard let f = from.first else { return }
        let moved = entries[f]
        var order = entries
        order.move(fromOffsets: from, toOffset: to)
        guard let i = order.firstIndex(where: { $0.entry == moved.entry && $0.title == moved.title }) else { return }
        let after = i == 0 ? nil : order[i - 1].entry
        entries = order
        run { try model.engine.menuMove(kind, entry: moved.entry, after: after) }
    }

    private func run(_ f: () throws -> Void) {
        do { try f(); error = nil } catch { self.error = error.localizedDescription }
        reload()
    }
}

/// A searchable list of what can be added to a menu.
private struct MenuChoicePicker: View {
    let choices: [MenuEntry]
    let pick: (MenuEntry) -> Void
    @State private var query = ""

    var body: some View {
        VStack(spacing: 6) {
            TextField("Search", text: $query).textFieldStyle(.roundedBorder).padding([.horizontal, .top], 10)
            List(choices.filter { query.isEmpty || $0.title.localizedCaseInsensitiveContains(query) }) { c in
                Button { pick(c) } label: { Text(c.entry == "---" ? "Separator" : c.title).frame(maxWidth: .infinity, alignment: .leading) }
                    .buttonStyle(.plain)
            }
            .listStyle(.plain)
        }
        .frame(width: 280, height: 340)
    }
}

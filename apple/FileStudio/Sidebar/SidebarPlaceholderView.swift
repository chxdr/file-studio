// SidebarPlaceholderView.swift — PLACEHOLDER for the sidebar (F-019: 200 pt,
// `.sidebar` material, filter field at the bottom). The window/sidebar/tabs
// session replaces it. It shows the wiring: favourites and volumes come from
// `AppModel` (kept fresh by `.favouritesChanged` / `.volumesChanged`), a
// click goes to the folder in the focused tab, and the context menu comes
// from the core (`MenuTarget.sidebarItem`).

import SwiftUI

struct SidebarPlaceholderView: View {
    let model: AppModel
    let window: WindowModel

    var body: some View {
        List {
            Section("Favourites") {
                ForEach(model.favourites) { f in
                    row(f.name, icon: "folder", path: f.path, kind: .favourite)
                }
            }
            Section("Locations") {
                ForEach(model.volumes) { v in
                    row(v.name, icon: v.isRemovable ? "externaldrive" : "internaldrive", path: v.mountPath, kind: .volume)
                }
            }
            if !model.ops.isEmpty {
                Section("Operations") { OpsPlaceholderView(model: model) }
            }
        }
        .listStyle(.sidebar)
        .frame(minWidth: 160)
    }

    private func row(_ title: String, icon: String, path: String, kind: SidebarItemKind) -> some View {
        Label(title, systemImage: icon)
            .contentShape(Rectangle())
            .onTapGesture {
                if let t = window.activeTabID.flatMap({ model.tabModel($0) }) { _ = try? t.api.go(path) }
            }
            .contextMenu {
                if let items = try? model.engine.contextMenu(.sidebarItem(path: path, kind: kind), context: MenuContext()) {
                    SwiftUIMenuItems(items: items, model: model)
                }
            }
    }
}

/// Draws core menu items in a SwiftUI context menu (commands and submenus).
struct SwiftUIMenuItems: View {
    let items: [MenuItem]
    let model: AppModel
    var body: some View {
        ForEach(Array(items.enumerated()), id: \.offset) { _, item in
            switch item {
            case .separator: Divider()
            case .command(let c):
                Button(c.checked ? "✓ " + c.title : c.title) { model.perform(c.invocation, via: .mouse) }
                    .disabled(!c.enabled)
            case .submenu(_, let title, let sub):
                Menu(title) { SwiftUIMenuItems(items: sub, model: model) }
            case .system: EmptyView()
            }
        }
    }
}

// PaneTabBar.swift — one pane's tabs (D-026 motion): a sliding selection
// pill, drag to reorder or to the other pane (or another window), close
// buttons on hover, middle-click to close, a + button and the core's tab
// context menu. Every change goes through the engine (`activateTab`,
// `moveTab`, `closeTab`, `tab.new`); the bar only draws `WindowModel.state`.

import SwiftUI
import UniformTypeIdentifiers

struct PaneTabBar: View {
    let model: AppModel
    let window: WindowModel
    let paneID: PaneID
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Namespace private var pill
    /// The gap a dragged tab would land in (0…count), for the insertion mark.
    @State private var dropGap: Int?

    static let height: CGFloat = 30
    nonisolated static let dragPrefix = "fstudio-tab:"

    private var pane: PaneState? { window.state.panes.first { $0.id == paneID } }
    private var isActivePane: Bool {
        window.state.panes.indices.contains(window.state.activePane) && window.state.panes[window.state.activePane].id == paneID
    }

    var body: some View {
        let tabs = pane?.tabs ?? []
        let active = pane?.activeTabID
        GeometryReader { geo in
            let avail = max(geo.size.width - 40, 1)
            let width = min(max(avail / CGFloat(max(tabs.count, 1)), 96), 240)
            HStack(spacing: 4) {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 2) {
                        ForEach(Array(tabs.enumerated()), id: \.element.id) { i, t in
                            TabItem(model: model, tab: t, selected: t.id == active, emphasised: isActivePane,
                                    showsGapBefore: dropGap == i, showsGapAfter: dropGap == tabs.count && i == tabs.count - 1,
                                    pill: pill)
                                .frame(width: width)
                                .onDrop(of: [.plainText], delegate: TabDropDelegate(
                                    index: i, width: width, tabs: tabs.map(\.id), paneID: paneID, model: model, gap: $dropGap))
                        }
                    }
                    .padding(.horizontal, 6)
                    .animation(reduceMotion ? nil : .spring(duration: 0.3, bounce: 0.18), value: active)
                    .animation(reduceMotion ? nil : .spring(duration: 0.3, bounce: 0.12), value: tabs.map(\.id))
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                // Drops past the last tab land at the end.
                .onDrop(of: [.plainText], delegate: TabDropDelegate(
                    index: tabs.count, width: 0, tabs: tabs.map(\.id), paneID: paneID, model: model, gap: $dropGap))
                Button {
                    newTab()
                } label: {
                    Image(systemName: "plus")
                        .font(.system(size: 12, weight: .medium))
                        .frame(width: 26, height: 22)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.borderless)
                .help("New Tab (⌘T)")
                .accessibilityLabel("New tab")
                .padding(.trailing, 8)
            }
            .frame(maxHeight: .infinity)
        }
        .frame(height: Self.height)
        .background(Color(nsColor: .windowBackgroundColor))
        .overlay(alignment: .bottom) { Rectangle().fill(Color(nsColor: .separatorColor)).frame(height: 1) }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Tabs")
    }

    private func newTab() {
        guard let t = pane?.activeTabID else { return }
        model.perform(Invocation(id: "tab.new", tab: t), via: .mouse, window: window.id)
    }
}

/// One tab: title, close-on-hover, the pill when selected.
private struct TabItem: View {
    let model: AppModel
    let tab: TabState
    let selected: Bool
    /// The tab's pane is the active pane: the pill uses full accent.
    let emphasised: Bool
    let showsGapBefore: Bool
    let showsGapAfter: Bool
    let pill: Namespace.ID
    @State private var hover = false
    @State private var hoverClose = false

    private var title: String { model.tabModel(tab.id)?.title ?? tab.title }
    private var location: String { model.tabModel(tab.id)?.location ?? tab.location }

    var body: some View {
        HStack(spacing: 5) {
            ZStack {
                Image(nsImage: IconCache.shared.icon(key: "folder", path: location))
                    .resizable()
                    .frame(width: 14, height: 14)
                    .opacity(hover ? 0 : 1)
                if hover {
                    Button {
                        close()
                    } label: {
                        Image(systemName: "xmark")
                            .font(.system(size: 8, weight: .bold))
                            .frame(width: 16, height: 16)
                            .background(Circle().fill(Color.primary.opacity(hoverClose ? 0.14 : 0)))
                            .contentShape(Circle())
                    }
                    .buttonStyle(.plain)
                    .foregroundStyle(.secondary)
                    .onHover { hoverClose = $0 }
                    .help("Close Tab (⌘W)")
                    .accessibilityLabel("Close \(title)")
                    .transition(.opacity)
                }
            }
            .frame(width: 16, height: 16)
            Text(title)
                .font(.system(size: 12, weight: selected ? .medium : .regular))
                .foregroundStyle(selected ? .primary : .secondary)
                .lineLimit(1)
                .truncationMode(.middle)
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 8)
        .frame(height: 24)
        .background {
            if selected {
                RoundedRectangle(cornerRadius: 6, style: .continuous)
                    .fill(Color.accentColor.opacity(emphasised ? 0.2 : 0.1))
                    .overlay {
                        RoundedRectangle(cornerRadius: 6, style: .continuous)
                            .strokeBorder(Color.accentColor.opacity(emphasised ? 0.55 : 0.25), lineWidth: 1)
                    }
                    .matchedGeometryEffect(id: "pill", in: pill)
            } else if hover {
                RoundedRectangle(cornerRadius: 6, style: .continuous).fill(Color.primary.opacity(0.06))
            }
        }
        .overlay(alignment: .leading) { if showsGapBefore { insertionMark.offset(x: -2) } }
        .overlay(alignment: .trailing) { if showsGapAfter { insertionMark.offset(x: 2) } }
        .contentShape(Rectangle())
        .onHover { h in withAnimation(Motion.fade) { hover = h } }
        .onTapGesture { activate() }
        .overlay { MiddleClickCatcher { close() } }
        .onDrag { NSItemProvider(object: "\(PaneTabBar.dragPrefix)\(tab.id)" as NSString) }
        .contextMenu {
            if let items = try? model.engine.contextMenu(.tab(tab.id), context: MenuContext()) {
                SwiftUIMenuItems(items: items, model: model)
            }
        }
        .help(location)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(title)
        .accessibilityValue(location)
        .accessibilityAddTraits(selected ? [.isButton, .isSelected] : .isButton)
        .accessibilityAction { activate() }
        .accessibilityAction(named: "Close tab") { close() }
    }

    private var insertionMark: some View {
        Capsule().fill(Color.accentColor).frame(width: 2, height: 18)
    }

    private func activate() {
        guard !selected else { return }
        do { try model.engine.activateTab(tab.id) } catch { model.statusMessage = error.localizedDescription }
    }

    private func close() {
        model.perform(Invocation(id: "tab.close", tab: tab.id), via: .mouse)
    }
}

/// Drops a dragged tab before or after the tab under the pointer.
private struct TabDropDelegate: DropDelegate {
    let index: Int
    /// The tab's width (0: the area after the last tab).
    let width: CGFloat
    let tabs: [TabID]
    let paneID: PaneID
    let model: AppModel
    @Binding var gap: Int?

    private func gapAt(_ info: DropInfo) -> Int {
        width == 0 ? tabs.count : (info.location.x < width / 2 ? index : index + 1)
    }

    func validateDrop(info: DropInfo) -> Bool { info.hasItemsConforming(to: [.plainText]) }
    func dropEntered(info: DropInfo) { gap = gapAt(info) }
    func dropUpdated(info: DropInfo) -> DropProposal? {
        gap = gapAt(info)
        return DropProposal(operation: .move)
    }
    func dropExited(info: DropInfo) { gap = nil }

    func performDrop(info: DropInfo) -> Bool {
        let target = gapAt(info)
        gap = nil
        guard let provider = info.itemProviders(for: [.plainText]).first else { return false }
        let tabs = tabs, paneID = paneID, model = model
        _ = provider.loadObject(ofClass: NSString.self) { obj, _ in
            guard let s = obj as? String, s.hasPrefix(PaneTabBar.dragPrefix),
                  let id = TabID(s.dropFirst(PaneTabBar.dragPrefix.count)) else { return }
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    // The core inserts after removing the tab, so a move to the
                    // right within the same pane lands one gap earlier.
                    var at = target
                    if let from = tabs.firstIndex(of: id), from < at { at -= 1 }
                    if tabs.firstIndex(of: id) == at { return }
                    do { try model.engine.moveTab(id, toPane: paneID, index: UInt32(at)) } catch {
                        model.statusMessage = error.localizedDescription
                    }
                }
            }
        }
        return true
    }
}

/// Draws core menu items in a SwiftUI context menu (commands and submenus;
/// the system submenus are AppKit-only and skipped here).
struct SwiftUIMenuItems: View {
    let items: [MenuItem]
    let model: AppModel
    var body: some View {
        ForEach(Array(items.enumerated()), id: \.offset) { _, item in
            switch item {
            case .separator: Divider()
            case .command(let c):
                if c.checked {
                    Toggle(c.title, isOn: Binding(get: { true }, set: { _ in model.perform(c.invocation, via: .mouse) }))
                        .disabled(!c.enabled)
                } else {
                    Button(c.title) { model.perform(c.invocation, via: .mouse) }.disabled(!c.enabled)
                }
            case .submenu(_, let title, let sub):
                Menu(title) { SwiftUIMenuItems(items: sub, model: model) }
            case .system: EmptyView()
            }
        }
    }
}

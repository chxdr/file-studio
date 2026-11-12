// ShortcutToasts.swift — drawing the held-modifier overlay and the
// "Did you know?" hint (ported from Import Studio's ShortcutToasts.swift).
// What to show comes from the core (`heldShortcuts`, `hintFor`); these views
// only draw and animate it, in the corners chosen in Settings. Under reduced
// motion everything fades.

import SwiftUI

/// The state the overlay host draws; `OverlayController` keeps it current.
@MainActor
@Observable
final class OverlayState {
    var held: HeldShortcuts?
    var hint: Hint?
}

/// Everything drawn over a window: held-modifier list, hint and toasts.
struct OverlayRoot: View {
    let model: AppModel
    let state: OverlayState
    let toasts: ToastCenter
    let appearance: AppearanceModel

    private var overlay: Settings.Overlay { model.settings.overlay }
    private var hints: Settings.Hints { model.settings.hints }
    /// Room for the toolbar above and the status line below.
    private let top: CGFloat = 64, bottom: CGFloat = 36, side: CGFloat = 16

    var body: some View {
        ZStack {
            if let held = state.held {
                HeldShortcutsView(held: held, corner: overlay.corner, appearance: appearance)
                    .padding(.top, top).padding(.bottom, bottom).padding(.horizontal, side)
                    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: overlay.corner.alignment)
                    .transition(slide(overlay.corner))
                    .allowsHitTesting(false)
            }
            if let hint = state.hint, state.held == nil {
                HintToast(model: model, hint: hint, onDismiss: { OverlayController.shared.dismissHint() })
                    .padding(.top, top).padding(.bottom, bottom).padding(.horizontal, side)
                    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: hints.corner.alignment)
                    .transition(slide(hints.corner))
                    .id(hint.id)
            }
            VStack(spacing: 8) {
                ForEach(toasts.toasts) { t in
                    ToastCard(toast: t, dismiss: { toasts.dismiss(t.id) })
                        .transition(appearance.transition(.move(edge: .bottom).combined(with: .opacity)))
                }
            }
            .padding(.bottom, bottom + 8)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .bottom)
        }
        .animation(appearance.animation(.spring(duration: 0.4, bounce: 0.2)), value: toasts.toasts)
    }

    private func slide(_ c: Corner) -> AnyTransition {
        appearance.transition(.asymmetric(
            insertion: .scale(scale: 0.85, anchor: c.unitPoint).combined(with: .offset(x: c.isLeading ? -24 : 24))
                .combined(with: .opacity),
            removal: .scale(scale: 0.92, anchor: c.unitPoint).combined(with: .opacity)))
    }
}

// MARK: - Held modifier

struct HeldShortcutsView: View {
    let held: HeldShortcuts
    let corner: Corner
    let appearance: AppearanceModel
    @State private var shown = 0
    @State private var scroll = ScrollMetrics()

    private var animate: Bool { appearance.effective == .full }

    var body: some View {
        let pill = HStack(spacing: 10) {
            Text(held.title)
                .font(.title3.weight(.semibold))
                .padding(.horizontal, 16).padding(.vertical, 10)
                .background(Capsule().fill(Color.accentColor.gradient))
                .foregroundStyle(.white)
                .shadow(color: Color.accentColor.opacity(0.35), radius: 10, y: 3)
                .contentTransition(.interpolate)
            Text("+").font(.title2.weight(.light)).foregroundStyle(.secondary)
        }
        HStack(alignment: corner.isTop ? .top : .bottom, spacing: 10) {
            if corner.isLeading { list; pill } else { pill; list }
        }
        .onAppear { stagger() }
        .onChange(of: held) { shown = held.rows.count }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Shortcuts with \(held.title)")
    }

    /// Rows with a category heading before each group.
    private var items: [(id: String, heading: String?, row: ShortcutRow?)] {
        var out: [(String, String?, ShortcutRow?)] = []
        var last: String?
        for r in held.rows {
            if r.category != last { out.append(("h:" + r.category, r.category, nil)); last = r.category }
            out.append((r.id, nil, r))
        }
        return out
    }

    private var list: some View {
        let rows = items
        return ScrollView(.vertical) {
            VStack(spacing: 8) {
                if rows.isEmpty {
                    Text("Nothing is bound to this key").font(.callout).foregroundStyle(.secondary).padding(12)
                }
                ForEach(Array(rows.enumerated()), id: \.element.id) { i, item in
                    Group {
                        if let h = item.heading {
                            Text(h.uppercased()).font(.caption2.weight(.semibold)).foregroundStyle(.secondary)
                                .frame(maxWidth: .infinity, alignment: .leading).padding(.leading, 4).padding(.top, i == 0 ? 0 : 4)
                        } else if let r = item.row {
                            KeyCard(keys: r.keys, label: r.label)
                        }
                    }
                    .opacity(i < shown || !animate ? 1 : 0)
                    .offset(y: i < shown || !animate ? 0 : (corner.isTop ? -10 : 10))
                }
            }
            .padding(10).padding(.trailing, 6)
        }
        .scrollIndicators(.never)
        .onScrollGeometryChange(for: ScrollMetrics.self) { g in
            ScrollMetrics(offset: g.contentOffset.y, content: g.contentSize.height, visible: g.containerSize.height)
        } action: { _, m in scroll = m }
        .frame(width: 300)
        .frame(maxHeight: 440)
        .fixedSize(horizontal: false, vertical: true)
        .mask {
            LinearGradient(stops: [.init(color: .clear, location: 0), .init(color: .black, location: 0.03),
                                   .init(color: .black, location: 0.97), .init(color: .clear, location: 1)],
                           startPoint: .top, endPoint: .bottom)
        }
        .overlay(alignment: .topTrailing) { ScrollBar(m: scroll).padding(.vertical, 10).padding(.trailing, 5) }
        .background(RoundedRectangle(cornerRadius: 16).fill(.regularMaterial))
        .overlay(RoundedRectangle(cornerRadius: 16).strokeBorder(Color.primary.opacity(0.1)))
        .shadow(color: .black.opacity(0.3), radius: 18, y: 6)
    }

    private func stagger() {
        guard animate else { shown = held.rows.count + 64; return }
        let n = items.count
        Task { @MainActor in
            for i in 0..<n {
                withAnimation(.spring(duration: 0.35, bounce: 0.2)) { shown = i + 1 }
                try? await Task.sleep(for: .milliseconds(18))
            }
        }
    }
}

struct ScrollMetrics: Equatable {
    var offset: CGFloat = 0
    var content: CGFloat = 0
    var visible: CGFloat = 0
    var overflows: Bool { content > visible + 1 }
}

/// A slim scroll bar that stays visible whenever the list is longer than the box.
private struct ScrollBar: View {
    let m: ScrollMetrics

    var body: some View {
        GeometryReader { geo in
            if m.overflows {
                let track = geo.size.height
                let thumb = max(28, track * m.visible / m.content)
                let range = max(1, m.content - m.visible)
                let y = (track - thumb) * min(1, max(0, m.offset / range))
                ZStack(alignment: .top) {
                    Capsule().fill(Color.primary.opacity(0.1))
                    Capsule().fill(Color.primary.opacity(0.5)).frame(height: thumb).offset(y: y)
                }
                .frame(width: 4)
                .frame(maxWidth: .infinity, alignment: .trailing)
            }
        }
        .frame(width: 4)
        .allowsHitTesting(false)
    }
}

private struct KeyCard: View {
    let keys: String
    let label: String

    var body: some View {
        HStack(spacing: 12) {
            KeyCap(text: keys, size: keys.count > 2 ? 15 : 22)
            Text(label).font(.callout).foregroundStyle(.primary).fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
        }
        .padding(8)
        .background(RoundedRectangle(cornerRadius: 11).fill(Color.primary.opacity(0.06)))
    }
}

/// A keyboard key: rounded, slightly raised.
struct KeyCap: View {
    let text: String
    var size: CGFloat = 22

    var body: some View {
        Text(text)
            .font(.system(size: size, weight: .semibold, design: .rounded))
            .lineLimit(1)
            .minimumScaleFactor(0.6)
            .padding(.horizontal, 8)
            .frame(minWidth: 46, minHeight: 46)
            .background {
                RoundedRectangle(cornerRadius: 9)
                    .fill(LinearGradient(colors: [Color.primary.opacity(0.12), Color.primary.opacity(0.05)],
                                         startPoint: .top, endPoint: .bottom))
                    .shadow(color: .black.opacity(0.25), radius: 0, y: 2)
            }
            .overlay(RoundedRectangle(cornerRadius: 9).strokeBorder(Color.primary.opacity(0.14)))
    }
}

// MARK: - Did you know?

struct HintToast: View {
    let model: AppModel
    let hint: Hint
    let onDismiss: () -> Void
    @State private var hover = false

    var body: some View {
        HStack(spacing: 12) {
            KeyCap(text: hint.keys, size: hint.keys.count > 2 ? 16 : 22)
            VStack(alignment: .leading, spacing: 2) {
                Label("Did you know?", systemImage: "lightbulb.fill")
                    .font(.caption.weight(.semibold)).foregroundStyle(.orange)
                Text("Press \(Text(hint.keys).bold()) for \(Text(hint.label))")
                    .font(.callout).fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 0)
            Menu {
                Button("Don’t Show Hints") { change { $0.hints.enabled = false }; onDismiss() }
                if model.settings.hints.frequency != .rarely {
                    Button("Show Hints Less Often") { change { $0.hints.frequency = .rarely }; onDismiss() }
                }
                Button("Hint Settings…") {
                    SettingsWindow.show(model: model, section: .shortcuts)
                    onDismiss()
                }
            } label: { Image(systemName: "ellipsis.circle") }
                .menuStyle(.borderlessButton).menuIndicator(.hidden).fixedSize()
                .opacity(hover ? 1 : 0.55)
            Button(action: onDismiss) { Image(systemName: "xmark") }
                .buttonStyle(.borderless).opacity(hover ? 1 : 0.55)
        }
        .padding(12)
        .frame(width: 350)
        .background(RoundedRectangle(cornerRadius: 14).fill(.regularMaterial))
        .overlay(RoundedRectangle(cornerRadius: 14).strokeBorder(Color.primary.opacity(0.1)))
        .shadow(color: .black.opacity(0.28), radius: 14, y: 5)
        .onHover { hover = $0 }
    }

    private func change(_ f: (inout Settings) -> Void) {
        var s = model.settings
        f(&s)
        do { try model.engine.updateSettings(s) } catch { ToastCenter.of(model).show(error.localizedDescription, style: .error) }
    }
}

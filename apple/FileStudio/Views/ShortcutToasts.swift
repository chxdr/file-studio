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
    /// The held list's scroll position and where its panel is (for the wheel).
    let heldScroll = HeldScrollState()
}

/// Scrolling the held-modifier list with the mouse wheel. The overlay is
/// click-through, so the host (`OverlayHost`) takes wheel events over the
/// panel itself and moves `offset`; the list is drawn shifted by it.
@MainActor
@Observable
final class HeldScrollState {
    var offset: CGFloat = 0
    /// The full height of the columns, and how much of it fits.
    var content: CGFloat = 0
    var visible: CGFloat = 0
    /// The list panel, in the overlay's coordinates (top-left origin); empty when hidden.
    var panelFrame: CGRect = .zero

    var metrics: ScrollMetrics { ScrollMetrics(offset: offset, content: content, visible: visible) }

    /// A wheel turn: `dy` > 0 scrolls up (towards the first row), as AppKit's deltas do.
    func scroll(by dy: CGFloat) { offset = HeldLayout.clamp(offset - dy, content: content, visible: visible) }
    func reset() { offset = 0 }
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
                GeometryReader { g in
                    HeldShortcutsView(held: held, corner: overlay.corner, appearance: appearance,
                                      available: CGSize(width: g.size.width - 2 * side, height: g.size.height - top - bottom),
                                      scroll: state.heldScroll)
                        .padding(.top, top).padding(.bottom, bottom).padding(.horizontal, side)
                        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: overlay.corner.alignment)
                }
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
        .coordinateSpace(.named(HeldShortcutsView.space))
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

/// How the held list is split into columns (pure; sizes are estimates, the
/// drawn heights are measured for scrolling).
enum HeldLayout {
    /// A column's width in the panel, and the cards' width inside it.
    static let columnWidth: CGFloat = 300
    static let cardWidth: CGFloat = 274
    static let gap: CGFloat = 10
    static let spacing: CGFloat = 8
    static let rowHeight: CGFloat = 62
    /// A compact card (smaller key cap), used when the full size would need scrolling.
    static let compactRowHeight: CGFloat = 46
    static let headingHeight: CGFloat = 18
    /// Room the "⌘ Command +" pill takes beside the list.
    static let pillWidth: CGFloat = 190
    static let maxColumns = 4

    /// How many columns fit beside the pill in `width`.
    static func columnsThatFit(width: CGFloat) -> Int {
        let n = Int(((width - pillWidth - gap) + gap) / (columnWidth + gap))
        return min(max(n, 1), maxColumns)
    }

    /// Splits items (true = a category heading) into at most `maxColumns`
    /// columns of about equal height, using as few as fit `maxHeight`. A
    /// heading never ends a column; a column that starts mid-category gets
    /// its heading repeated (marked `continued`).
    static func columns(_ headings: [Bool], maxHeight: CGFloat, maxColumns: Int,
                        rowHeight: CGFloat = rowHeight) -> [[Slot]] {
        guard !headings.isEmpty else { return [[]] }
        let heights = headings.map { ($0 ? headingHeight : rowHeight) + spacing }
        let total = heights.reduce(0, +)
        let wanted = Int((total / max(maxHeight, 1)).rounded(.up))
        let n = min(max(wanted, 1), max(maxColumns, 1))
        let target = total / CGFloat(n)
        var cols: [[Slot]] = [[]]
        var h: CGFloat = 0
        var lastHeading: Int?
        for (i, isHeading) in headings.enumerated() {
            if isHeading { lastHeading = i }
            let next = heights[i] + (isHeading && i + 1 < heights.count ? heights[i + 1] : 0)
            if !cols[cols.count - 1].isEmpty, cols.count < n, h + next > target + rowHeight / 2 {
                cols.append([])
                h = 0
                if !isHeading, let lh = lastHeading {
                    cols[cols.count - 1].append(Slot(index: lh, continued: true))
                    h += heights[lh]
                }
            }
            cols[cols.count - 1].append(Slot(index: i, continued: false))
            h += heights[i]
        }
        return cols
    }

    /// The estimated height of the tallest column.
    static func height(_ cols: [[Slot]], _ headings: [Bool], rowHeight: CGFloat = rowHeight) -> CGFloat {
        cols.map { $0.reduce(0) { $0 + (headings[$1.index] ? headingHeight : rowHeight) + spacing } }.max() ?? 0
    }

    /// Full-size cards, unless only compact ones fit without scrolling.
    static func compact(_ headings: [Bool], maxHeight: CGFloat, maxColumns: Int) -> Bool {
        let full = columns(headings, maxHeight: maxHeight, maxColumns: maxColumns)
        return height(full, headings) > maxHeight
    }

    /// One place in a column: an item, or a repeated heading.
    struct Slot: Equatable {
        var index: Int
        var continued: Bool
    }

    /// The scroll offset kept inside the content.
    static func clamp(_ offset: CGFloat, content: CGFloat, visible: CGFloat) -> CGFloat {
        min(max(offset, 0), max(content - visible, 0))
    }
}

struct HeldShortcutsView: View {
    /// The coordinate space the panel frame is reported in (the overlay's).
    static let space = "heldOverlay"
    let held: HeldShortcuts
    let corner: Corner
    let appearance: AppearanceModel
    /// Room for the pill and the list (the window less the toolbar and status line).
    let available: CGSize
    let scroll: HeldScrollState
    /// The tour's demo: no stagger, a fixed size.
    var animated = true
    @State private var shown = 0

    private var animate: Bool { animated && appearance.effective == .full }
    /// The list's visible height inside its panel (10 pt padding above and below).
    private var maxListHeight: CGFloat { max(available.height - 20, HeldLayout.rowHeight) }

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
        .onAppear { scroll.reset(); stagger() }
        .onChange(of: held) { shown = items.count + 64 }
        .onChange(of: held.title) { scroll.reset() }
        .onDisappear { scroll.panelFrame = .zero }
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
        let headings = rows.map { $0.heading != nil }
        let fit = HeldLayout.columnsThatFit(width: available.width)
        let compact = HeldLayout.compact(headings, maxHeight: maxListHeight, maxColumns: fit)
        let cols = HeldLayout.columns(headings, maxHeight: maxListHeight, maxColumns: fit,
                                      rowHeight: compact ? HeldLayout.compactRowHeight : HeldLayout.rowHeight)
        let m = scroll.metrics
        return HStack(alignment: .top, spacing: HeldLayout.gap) {
            if rows.isEmpty {
                Text("Nothing is bound to this key").font(.callout).foregroundStyle(.secondary).padding(12)
            }
            ForEach(cols.indices, id: \.self) { c in
                VStack(spacing: HeldLayout.spacing) {
                    ForEach(Array(cols[c].enumerated()), id: \.offset) { n, slot in
                        let item = rows[slot.index]
                        Group {
                            if let h = item.heading {
                                Text(h.uppercased() + (slot.continued ? " (CONTINUED)" : ""))
                                    .font(.caption2.weight(.semibold)).foregroundStyle(.secondary)
                                    .frame(maxWidth: .infinity, alignment: .leading)
                                    .padding(.leading, 4).padding(.top, n == 0 ? 0 : 4)
                            } else if let r = item.row {
                                KeyCard(keys: r.keys, label: r.label, compact: compact)
                            }
                        }
                        .opacity(slot.index < shown || !animate ? 1 : 0)
                        .offset(y: slot.index < shown || !animate ? 0 : (corner.isTop ? -10 : 10))
                    }
                }
                .frame(width: HeldLayout.cardWidth)
            }
        }
        .fixedSize(horizontal: false, vertical: true)
        .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { h in
            scroll.content = h
            scroll.visible = min(h, maxListHeight)
            scroll.offset = HeldLayout.clamp(scroll.offset, content: h, visible: scroll.visible)
        }
        .offset(y: -m.offset)
        .frame(height: min(m.content > 0 ? m.content : maxListHeight, maxListHeight), alignment: .top)
        .clipped()
        .padding(10).padding(.trailing, 6)
        .mask {
            LinearGradient(stops: [.init(color: .clear, location: 0), .init(color: .black, location: m.offset > 0 ? 0.04 : 0.015),
                                   .init(color: .black, location: m.overflows ? 0.96 : 0.985), .init(color: .clear, location: 1)],
                           startPoint: .top, endPoint: .bottom)
        }
        .overlay(alignment: .topTrailing) { ScrollBar(m: m).padding(.vertical, 10).padding(.trailing, 5) }
        .overlay(alignment: .bottom) {
            if m.overflows, m.offset < 1 {
                Label("Scroll for more", systemImage: "computermouse")
                    .font(.caption.weight(.medium)).foregroundStyle(.secondary)
                    .padding(.horizontal, 10).padding(.vertical, 4)
                    .background(Capsule().fill(.thickMaterial))
                    .padding(.bottom, 6)
                    .transition(.opacity)
            }
        }
        .background(RoundedRectangle(cornerRadius: 16).fill(.regularMaterial))
        .overlay(RoundedRectangle(cornerRadius: 16).strokeBorder(Color.primary.opacity(0.1)))
        .shadow(color: .black.opacity(0.3), radius: 18, y: 6)
        .onGeometryChange(for: CGRect.self) { $0.frame(in: .named(Self.space)) } action: { scroll.panelFrame = $0 }
    }

    private func stagger() {
        let n = items.count
        guard animate else { shown = n + 64; return }
        Task { @MainActor in
            for i in 0..<n {
                withAnimation(.spring(duration: 0.35, bounce: 0.2)) { shown = i + 1 }
                try? await Task.sleep(for: .milliseconds(12))
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
    var compact = false

    var body: some View {
        HStack(spacing: 12) {
            KeyCap(text: keys, size: (keys.count > 2 ? 15 : 22) * (compact ? 0.8 : 1), side: compact ? 30 : 46)
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
    /// The cap's minimum width and height.
    var side: CGFloat = 46

    var body: some View {
        Text(text)
            .font(.system(size: size, weight: .semibold, design: .rounded))
            .lineLimit(1)
            .minimumScaleFactor(0.6)
            .padding(.horizontal, 8)
            .frame(minWidth: side, minHeight: side)
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

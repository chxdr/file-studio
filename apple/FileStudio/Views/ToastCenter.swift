// ToastCenter.swift — one place to show short messages (undo refusals,
// errors, "Cleaned up paste: removed quotes" chips…). Any code may call
//
//     ToastCenter.of(model).show("Nothing to undo", style: .warning)
//
// The overlay host (OverlayHost.swift) draws them in the bottom centre of the
// key window. `ToastCard` is the reusable look, also usable on its own.

import Observation
import SwiftUI

struct Toast: Identifiable, Equatable {
    enum Style: Equatable { case info, success, warning, error }
    let id = UUID()
    var text: String
    var detail: String?
    var style: Style
    var seconds: Double
    /// An optional button ("Undo", "Settings…").
    var actionTitle: String?
    var action: (@MainActor () -> Void)?
    static func == (a: Toast, b: Toast) -> Bool { a.id == b.id }
}

@MainActor
@Observable
final class ToastCenter {
    private(set) var toasts: [Toast] = []
    @ObservationIgnored private var timers: [UUID: Task<Void, Never>] = [:]

    static func of(_ m: AppModel) -> ToastCenter { m.attachment(ToastCenter.self) { ToastCenter() } }

    /// Shows a toast; the same text twice in a row replaces the first.
    @discardableResult
    func show(_ text: String, detail: String? = nil, style: Toast.Style = .info, seconds: Double = 3.5,
              actionTitle: String? = nil, action: (@MainActor () -> Void)? = nil) -> UUID {
        if let old = toasts.first(where: { $0.text == text && $0.detail == detail }) { dismiss(old.id) }
        let t = Toast(text: text, detail: detail, style: style, seconds: seconds, actionTitle: actionTitle, action: action)
        toasts.append(t)
        if toasts.count > 3 { dismiss(toasts[0].id) }
        timers[t.id] = Task { @MainActor [weak self] in
            try? await Task.sleep(for: .seconds(seconds))
            guard !Task.isCancelled else { return }
            self?.dismiss(t.id)
        }
        return t.id
    }

    func dismiss(_ id: UUID) {
        timers[id]?.cancel()
        timers[id] = nil
        toasts.removeAll { $0.id == id }
    }
}

/// The toast look: a rounded card with an icon, text and optional button.
struct ToastCard: View {
    let toast: Toast
    var dismiss: () -> Void = {}

    private var icon: (String, Color) {
        switch toast.style {
        case .info: ("info.circle.fill", .accentColor)
        case .success: ("checkmark.circle.fill", .green)
        case .warning: ("exclamationmark.triangle.fill", .orange)
        case .error: ("xmark.octagon.fill", .red)
        }
    }

    var body: some View {
        HugWidth(minWidth: 200, maxWidth: 480, maxFraction: 0.6) {
            HStack(alignment: .center, spacing: 10) {
                Image(systemName: icon.0).foregroundStyle(icon.1).font(.system(size: 15))
                VStack(alignment: .leading, spacing: 2) {
                    // Long text wraps to two lines; a long file name is cut in
                    // the middle. The whole text is in the tooltip and for VoiceOver.
                    Text(toast.text).font(.callout).lineLimit(2).truncationMode(.middle)
                        .fixedSize(horizontal: false, vertical: true)
                    if let d = toast.detail {
                        Text(d).font(.caption).foregroundStyle(.secondary).lineLimit(3)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
                .layoutPriority(1)
                // Only grows when the minimum width is wider than the content.
                Spacer(minLength: 0)
                if let t = toast.actionTitle, let a = toast.action {
                    Button(t) { a(); dismiss() }.buttonStyle(.link).font(.callout).fixedSize()
                }
                Button(action: dismiss) { Image(systemName: "xmark").font(.system(size: 10, weight: .semibold)) }
                    .buttonStyle(.plain).foregroundStyle(.secondary).fixedSize()
                    .accessibilityLabel("Close")
            }
            .padding(.horizontal, 14).padding(.vertical, 9)
        }
        .background(RoundedRectangle(cornerRadius: 12).fill(.regularMaterial))
        .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(Color.primary.opacity(0.1)))
        .shadow(color: .black.opacity(0.25), radius: 12, y: 4)
        .help(toast.text)
        .accessibilityElement(children: .contain)
        .accessibilityLabel([toast.text, toast.detail].compactMap { $0 }.joined(separator: ". "))
    }
}

/// Sizes its content to the content's own width, kept between `minWidth`
/// and the smaller of `maxWidth` and `maxFraction` of the space offered, so
/// a toast hugs its text instead of filling a wide capsule. Past the cap the
/// content gets the capped width and wraps.
struct HugWidth: Layout {
    var minWidth: CGFloat
    var maxWidth: CGFloat
    var maxFraction: CGFloat

    private func width(_ proposal: ProposedViewSize, _ sub: LayoutSubview) -> CGFloat {
        let ideal = sub.sizeThatFits(.unspecified).width
        var cap = maxWidth
        if let w = proposal.width, w.isFinite { cap = min(cap, max(minWidth, w * maxFraction)) }
        return min(max(ideal, minWidth), cap).rounded(.up)
    }

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        guard let sub = subviews.first else { return .zero }
        let w = width(proposal, sub)
        return CGSize(width: w, height: sub.sizeThatFits(ProposedViewSize(width: w, height: nil)).height)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        subviews.first?.place(at: bounds.origin, anchor: .topLeading,
                              proposal: ProposedViewSize(width: bounds.width, height: bounds.height))
    }
}

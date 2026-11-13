// TourView.swift — the power tour sheet: a step pill, one card per signature
// feature (a demo drawn by the feature’s own views, the title, two lines, and the keys to press as
// key caps read from the core's current bindings), and Close / Back / Next.
// Motion follows setup (Import Studio's D-026): spring card transitions in
// the direction of travel and a sliding step pill; fades only under Reduce
// Motion, where the demos also hold still. ←/→ and Return move, Esc closes.

import SwiftUI

struct TourView: View {
    let model: AppModel
    let state: TourState
    let close: () -> Void
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Namespace private var stepPill

    var body: some View {
        VStack(spacing: 0) {
            stepIndicator
                .padding(.horizontal, 20)
                .padding(.top, 16)
                .padding(.bottom, 12)
            Divider()
            ZStack {
                TourCardView(model: model, card: state.card, frozenPhase: state.frozenPhase)
                    .id(state.index)
                    .transition(cardTransition)
                    .padding(.horizontal, 28)
                    .padding(.vertical, 20)
                    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            }
            .clipped()
            Divider()
            HStack {
                Button("Close Tour", action: close)
                    .keyboardShortcut(.cancelAction)
                    .help("Esc. The tour is always in Help › File Studio Power Tour")
                Spacer()
                if state.isLast {
                    Button("Open Settings › Shortcuts") { TourController.openShortcutSettings(model: model) }
                }
                Button { TourController.go(to: state.index - 1) } label: { Label("Back", systemImage: "chevron.left") }
                    .keyboardShortcut(.leftArrow, modifiers: [])
                    .disabled(state.index == 0)
                Button {
                    if state.isLast { close() } else { TourController.go(to: state.index + 1) }
                } label: {
                    HStack(spacing: 6) {
                        Text(state.isLast ? "Done" : "Next")
                        Image(systemName: state.isLast ? "checkmark.circle.fill" : "chevron.right")
                            .contentTransition(.symbolEffect(.replace))
                    }
                }
                .keyboardShortcut(.defaultAction)
                .buttonStyle(.borderedProminent)
                .controlSize(.large)
                // → as well as Return.
                Button("") { if !state.isLast { TourController.go(to: state.index + 1) } }
                    .keyboardShortcut(.rightArrow, modifiers: [])
                    .frame(width: 0, height: 0).opacity(0)
                    .accessibilityHidden(true)
            }
            .padding(16)
        }
        .frame(width: 760, height: 640)
        .accessibilityElement(children: .contain)
        .accessibilityLabel("File Studio Power Tour")
    }

    // MARK: Step pill (as setup's: the current step is named, the rest are numbered)

    private var stepIndicator: some View {
        let cards = state.cards
        return HStack(spacing: 4) {
            ForEach(cards.indices, id: \.self) { i in
                HStack(spacing: 5) {
                    ZStack {
                        Circle().fill(i < state.index ? Color.accentColor : Color.secondary.opacity(0.18))
                            .frame(width: 18, height: 18)
                        if i < state.index {
                            Image(systemName: "checkmark")
                                .font(.system(size: 9, weight: .bold))
                                .foregroundStyle(.white)
                                .transition(.scale.combined(with: .opacity))
                        } else {
                            Text("\(i + 1)").font(.system(size: 10, weight: .semibold))
                                .foregroundStyle(i == state.index ? .primary : .secondary)
                        }
                    }
                    if i == state.index {
                        Text(cards[i].short)
                            .font(.callout.weight(.semibold))
                            .lineLimit(1)
                            .fixedSize()
                            .transition(.opacity)
                    }
                }
                .padding(.horizontal, 8)
                .padding(.vertical, 5)
                .background {
                    if i == state.index {
                        Capsule().fill(Color.accentColor.opacity(0.16))
                            .matchedGeometryEffect(id: "step", in: stepPill)
                    }
                }
                .contentShape(Capsule())
                .onTapGesture { TourController.go(to: i) }
                .accessibilityElement(children: .ignore)
                .accessibilityLabel("Card \(i + 1) of \(cards.count), \(cards[i].short)"
                                    + (i < state.index ? ", seen" : i == state.index ? ", current" : ""))
                .accessibilityAddTraits(.isButton)
                .accessibilityAction { TourController.go(to: i) }
                if i < cards.count - 1 {
                    Capsule()
                        .fill(i < state.index ? Color.accentColor.opacity(0.7) : Color.secondary.opacity(0.2))
                        .frame(width: 10, height: 2)
                        .accessibilityHidden(true)
                }
            }
        }
        .frame(maxWidth: .infinity)
    }

    private var cardTransition: AnyTransition {
        guard !reduceMotion else { return .opacity }
        return .asymmetric(
            insertion: .move(edge: state.forward ? .trailing : .leading).combined(with: .opacity),
            removal: .move(edge: state.forward ? .leading : .trailing).combined(with: .opacity))
    }
}

/// One card: demo, title, two lines, keys. Rises in like setup's pages.
private struct TourCardView: View {
    let model: AppModel
    let card: TourCard
    let frozenPhase: Double?
    @State private var shown = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        let keysFor = { (id: String) in model.engine.commandKeys(id) }
        VStack(alignment: .leading, spacing: 12) {
            TourDemoStage(demo: card.demo, keysFor: keysFor, model: model, frozenPhase: frozenPhase)
                .frame(height: 300)
                .frame(maxWidth: .infinity)
                .accessibilityElement(children: .ignore)
                .accessibilityLabel("Demo")
                .accessibilityValue(card.demoDescription)
                .accessibilityAddTraits(.isImage)
            HStack(spacing: 10) {
                Image(systemName: card.symbol).font(.title2).foregroundStyle(.tint).frame(width: 28)
                    .accessibilityHidden(true)
                Text(card.title).font(.title2.bold())
                    .accessibilityAddTraits(.isHeader)
            }
            .opacity(shown ? 1 : 0).offset(y: shown || reduceMotion ? 0 : 8)
            Text(card.text(keysFor: keysFor, titleFor: title(of:)))
                .font(.body).foregroundStyle(.secondary)
                .lineLimit(3)
                .fixedSize(horizontal: false, vertical: true)
                .opacity(shown ? 1 : 0).offset(y: shown || reduceMotion ? 0 : 8)
            TourKeyRow(keys: card.keys, keysFor: keysFor)
                .opacity(shown ? 1 : 0)
        }
        .onAppear { withAnimation(.easeOut(duration: 0.3).delay(0.08)) { shown = true } }
    }

    private func title(of id: String) -> String {
        model.engine.commands().first { $0.id == id }?.title ?? id
    }
}

/// The keys to press: key caps from the current bindings, each with a caption.
private struct TourKeyRow: View {
    let keys: [TourCard.Key]
    let keysFor: (String) -> [String]

    var body: some View {
        HStack(alignment: .top, spacing: 18) {
            ForEach(keys.indices, id: \.self) { i in
                let k = keys[i]
                let caps = TourKeys.caps(k, keysFor: keysFor)
                VStack(alignment: .leading, spacing: 5) {
                    HStack(spacing: 5) {
                        if caps.isEmpty {
                            Text("no key yet").font(.callout.italic()).foregroundStyle(.secondary)
                                .frame(height: 28)
                        }
                        ForEach(caps.indices, id: \.self) { j in
                            if j > 0 { Text("or").font(.caption).foregroundStyle(.secondary) }
                            switch k {
                            case .typed: TypedChip(text: caps[j])
                            default: TourKeyCap(text: caps[j])
                            }
                        }
                    }
                    Text(caption(k)).font(.caption).foregroundStyle(.secondary)
                }
                .accessibilityElement(children: .ignore)
                .accessibilityLabel(spoken(k, caps))
            }
            Spacer(minLength: 0)
        }
    }

    private func caption(_ k: TourCard.Key) -> String {
        switch k {
        case .command(_, let c), .typed(_, let c), .literal(_, let c): c
        }
    }

    private func spoken(_ k: TourCard.Key, _ caps: [String]) -> String {
        switch k {
        case .typed(let t, let c): "Type \(t): \(c)"
        case .literal(let t, let c): "\(t.replacingOccurrences(of: "⌘", with: "Command")): \(c)"
        case .command(_, let c):
            caps.isEmpty ? "\(c): no key yet" : "\(caps.map(TourKeys.spoken).joined(separator: ", or ")): \(c)"
        }
    }
}

/// A compact key cap (the overlay's `KeyCap`, sized for a row of them).
struct TourKeyCap: View {
    let text: String
    var pressed = false

    var body: some View {
        Text(text)
            .font(.system(size: 14, weight: .semibold, design: .rounded))
            .lineLimit(1)
            .padding(.horizontal, 8)
            .frame(minWidth: 30, minHeight: 28)
            .background {
                RoundedRectangle(cornerRadius: 7)
                    .fill(pressed ? AnyShapeStyle(Color.accentColor.opacity(0.35))
                          : AnyShapeStyle(LinearGradient(colors: [Color.primary.opacity(0.13), Color.primary.opacity(0.05)],
                                                         startPoint: .top, endPoint: .bottom)))
                    .shadow(color: .black.opacity(pressed ? 0 : 0.25), radius: 0, y: pressed ? 0 : 1.5)
            }
            .overlay(RoundedRectangle(cornerRadius: 7).strokeBorder(Color.primary.opacity(0.14)))
            .offset(y: pressed ? 1 : 0)
    }
}

/// Text the user types (not a key).
struct TypedChip: View {
    let text: String

    var body: some View {
        Text(text)
            .font(.system(size: 14, weight: .medium, design: .monospaced))
            .padding(.horizontal, 9)
            .frame(minHeight: 28)
            .background(RoundedRectangle(cornerRadius: 7).strokeBorder(Color.accentColor.opacity(0.6),
                                                                        style: StrokeStyle(lineWidth: 1, dash: [3, 2])))
    }
}

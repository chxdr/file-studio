// OpsPlaceholderView.swift — the toolbar item of the operation queue: a
// progress ring (overall progress while ops run), a badge for conflicts or
// failures, and the queue popover. The type name and `init(model:)` are the
// contract with the window layer; the body is the real ops UI now.

import SwiftUI

struct OpsPlaceholderView: View {
    let model: AppModel
    @State private var ops: OpsModel
    @State private var appearance: AppearanceModel

    init(model: AppModel) {
        self.model = model
        _ops = State(initialValue: OpsModel.of(model))
        _appearance = State(initialValue: AppearanceModel.of(model))
    }

    var body: some View {
        @Bindable var o = ops
        Button { o.popoverShown.toggle() } label: {
            OpsRing(fraction: ops.overallFraction, running: !ops.active.isEmpty,
                    allPaused: !ops.active.isEmpty && ops.active.allSatisfy { $0.state == .paused },
                    conflicts: ops.conflictCount, failures: ops.failedCount, interrupted: ops.interrupted.count,
                    hasFinished: !ops.finished.isEmpty)
                .animation(appearance.animation(.smooth(duration: 0.3)), value: ops.overallFraction)
        }
        .buttonStyle(.plain)
        .help(ops.statusText ?? "Operations")
        .accessibilityLabel("Operations")
        .accessibilityValue(ops.statusText ?? "Idle")
        .popover(isPresented: $o.popoverShown, arrowEdge: .bottom) {
            OpsQueueView(ops: ops)
        }
    }
}

/// The ring itself: track, progress arc, centre glyph and badge.
struct OpsRing: View {
    let fraction: Double
    let running: Bool
    let allPaused: Bool
    let conflicts: Int
    let failures: Int
    let interrupted: Int
    let hasFinished: Bool

    var body: some View {
        ZStack {
            Circle().stroke(Color.primary.opacity(running ? 0.15 : 0.0), lineWidth: 3)
            if running {
                Circle().trim(from: 0, to: max(0.04, fraction))
                    .stroke(Color.accentColor, style: StrokeStyle(lineWidth: 3, lineCap: .round))
                    .rotationEffect(.degrees(-90))
            }
            Image(systemName: glyph)
                .font(.system(size: running ? 9 : 15, weight: .semibold))
                .foregroundStyle(running ? Color.accentColor : Color.secondary)
        }
        .frame(width: 22, height: 22)
        .padding(3)
        .overlay(alignment: .topTrailing) { badge }
        .contentShape(Rectangle())
    }

    private var glyph: String {
        if running { return allPaused ? "pause.fill" : "arrow.down" }
        return hasFinished ? "checkmark.circle" : "tray"
    }

    @ViewBuilder private var badge: some View {
        if conflicts > 0 {
            Badge(text: "\(conflicts)", color: .orange)
        } else if failures > 0 {
            Badge(text: "!", color: .red)
        } else if interrupted > 0 {
            Badge(text: "\(interrupted)", color: .blue)
        }
    }

    private struct Badge: View {
        let text: String
        let color: Color
        var body: some View {
            Text(text).font(.system(size: 9, weight: .bold)).foregroundStyle(.white)
                .frame(minWidth: 13, minHeight: 13)
                .background(Capsule().fill(color))
                .offset(x: 4, y: -3)
        }
    }
}

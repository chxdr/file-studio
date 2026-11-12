// PaneStatusLine.swift — the 22 pt line under each pane: item count,
// selection count and size, free space, and (in the active pane) the queue
// summary, pending keys and the last status message. Counts come from the
// core (`TabModel`, `AppModel.ops`, `AppModel.volumes`); this only spells them.

import SwiftUI

/// What a pane's SwiftUI parts (status line) read; updated by `PaneViewController`.
@MainActor
@Observable
final class PaneViewState {
    var tab: TabModel?
    var isActive = false
    var isDual = false
}

struct PaneStatusLine: View {
    let model: AppModel
    let state: PaneViewState

    static let height: CGFloat = 22

    var body: some View {
        HStack(spacing: 6) {
            if let t = state.tab {
                Text(left(t)).lineLimit(1).truncationMode(.middle)
                if let e = t.listing.error, t.listing.state == .error {
                    Text(e).foregroundStyle(.red).lineLimit(1)
                }
            }
            Spacer(minLength: 8)
            if state.isActive {
                if let k = model.pendingKeys {
                    Text(k).monospaced().foregroundStyle(.primary)
                        .padding(.horizontal, 5)
                        .background(RoundedRectangle(cornerRadius: 4).fill(Color.primary.opacity(0.08)))
                }
                if let m = model.statusMessage {
                    Text(m).lineLimit(1).transition(.opacity)
                }
                if let q = queueSummary {
                    HStack(spacing: 4) {
                        ProgressView(value: q.fraction).progressViewStyle(.circular).controlSize(.mini)
                        Text(q.text).lineLimit(1)
                    }
                    .accessibilityElement(children: .combine)
                }
            }
            if let t = state.tab, let v = PaneText.volume(for: t.location, in: model.volumes), v.totalBytes > 0 {
                Text("\(PaneText.bytes(v.freeBytes)) available").lineLimit(1)
            }
        }
        .font(.system(size: 11).monospacedDigit())
        .foregroundStyle(.secondary)
        .padding(.horizontal, 10)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Color(nsColor: .windowBackgroundColor))
        .overlay(alignment: .top) { Rectangle().fill(Color(nsColor: .separatorColor)).frame(height: 1) }
        .animation(Motion.fade, value: model.statusMessage)
        .task(id: state.isActive ? model.statusMessage : nil) {
            // Status messages fade after a few seconds (the active pane clears them).
            guard let m = model.statusMessage, state.isActive else { return }
            try? await Task.sleep(for: .seconds(5))
            if model.statusMessage == m { model.statusMessage = nil }
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel("Status")
    }

    /// "12 items" or "3 of 12 selected, 4.2 MB".
    private func left(_ t: TabModel) -> String {
        _ = t.selectionVersion
        let n = t.listing.count
        if t.listing.state == .loading && n == 0 { return "Loading…" }
        let sel = t.selectionCount
        guard sel > 0 else { return PaneText.items(n) }
        var s = "\(sel.formatted()) of \(PaneText.items(n)) selected"
        if let bytes = SelectionSize.of(t) { s += ", \(PaneText.bytes(bytes))" }
        return s
    }

    private var queueSummary: (text: String, fraction: Double)? {
        let live = model.ops.filter { !$0.state.isFinished }
        guard !live.isEmpty else { return nil }
        let done = live.reduce(UInt64(0)) { $0 + $1.bytesDone }, total = live.reduce(UInt64(0)) { $0 + $1.bytesTotal }
        let fraction = total > 0 ? Double(done) / Double(total) : live.map(\.fraction).reduce(0, +) / Double(live.count)
        let text = live.count == 1 ? live[0].title : "\(live.count) operations"
        let waiting = live.contains { $0.state == .waitingForDecision }
        return (waiting ? "\(text): waiting for you" : "\(text) · \(Int(fraction * 100))%", fraction)
    }
}

/// Sums the sizes of the selected files (folders count as nothing until the
/// core measures them). Cached per tab, listing version and selection.
@MainActor
enum SelectionSize {
    private static var cache: [TabID: (key: String, bytes: UInt64?)] = [:]
    /// Beyond this many selected rows the size is left out (keeps it instant).
    static let limit = 5_000

    static func of(_ t: TabModel) -> UInt64? {
        let key = "\(t.listing.version)/\(t.selectionVersion)"
        if let c = cache[t.id], c.key == key { return c.bytes }
        let rows = t.api.selection().rows.sorted()
        var bytes: UInt64? = nil
        if !rows.isEmpty && rows.count <= limit {
            var total: UInt64 = 0, any = false
            var i = 0
            // Fetch contiguous runs at once.
            while i < rows.count {
                var j = i
                while j + 1 < rows.count && rows[j + 1] == rows[j] + 1 && j - i < 255 { j += 1 }
                for r in t.api.rows(start: rows[i], count: rows[j] - rows[i] + 1) where r.kind != .folder {
                    total &+= r.size
                    any = true
                }
                i = j + 1
            }
            bytes = any ? total : nil
        }
        cache[t.id] = (key, bytes)
        return bytes
    }
}

// StatusLinePlaceholderView.swift — PLACEHOLDER status line under a pane:
// item and selection counts from `TabModel`, plus the last status message.

import SwiftUI

struct StatusLinePlaceholderView: View {
    let model: AppModel
    let tab: TabModel?

    var body: some View {
        HStack(spacing: 8) {
            if let t = tab {
                Text(t.listing.state == .loading ? "Loading…" : "\(t.listing.count) items")
                if t.selectionCount > 0 { Text("· \(t.selectionCount) selected") }
                if let e = t.listing.error { Text("· \(e)").foregroundStyle(.red) }
            }
            Spacer()
            if let k = model.pendingKeys { Text(k).monospaced() }
            if let m = model.statusMessage { Text(m).lineLimit(1) }
            Text(model.engine.implementationName).foregroundStyle(.tertiary)
        }
        .font(.system(size: 11))
        .foregroundStyle(.secondary)
        .padding(.horizontal, 10)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Color(nsColor: .windowBackgroundColor))
    }
}

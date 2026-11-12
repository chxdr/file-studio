// StatusLinePlaceholderView.swift — the status line under a pane: item and
// selection counts from `TabModel`, the running operation ("Copying… 42 %",
// or nothing when idle), the pending key sequence and the last message.
// The type name and `init(model:tab:)` are the contract with the window layer.

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
            if let t = OpsModel.of(model).statusText {
                HStack(spacing: 5) {
                    ProgressView().controlSize(.mini)
                    Text(t).monospacedDigit()
                }
                .onTapGesture { OpsModel.of(model).popoverShown = true }
            }
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

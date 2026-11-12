// OpsPlaceholderView.swift — PLACEHOLDER for the operation queue UI (PROPOSAL
// §10: toolbar progress ring, popover queue, conflict sheet). The ops-UI
// session replaces it. It shows the wiring: `AppModel.ops` is kept current
// from `.opProgress` / `.opFinished`; controls call the engine.

import SwiftUI

struct OpsPlaceholderView: View {
    let model: AppModel
    var body: some View {
        ForEach(model.ops.suffix(5)) { op in
            VStack(alignment: .leading, spacing: 2) {
                Text(op.title).font(.caption).lineLimit(1)
                ProgressView(value: op.fraction).controlSize(.small)
                if !op.conflicts.isEmpty {
                    Button("Keep Both") {
                        if let c = op.conflicts.first {
                            try? model.engine.resolveConflict(op: op.id, conflict: c.id, decision: .keepBoth, applyToAll: false)
                        }
                    }
                    .controlSize(.small)
                }
            }
        }
    }
}

// OpsQueueView.swift — the popover behind the toolbar ring: the queue with
// progress, speed and ETA per op, pause/resume/cancel, drag to reorder,
// failed items per file, finished ops until cleared, and interrupted ops
// from an earlier run.

import SwiftUI

struct OpsQueueView: View {
    let ops: OpsModel

    var body: some View {
        OpsQueueContent(active: ops.active, finished: ops.finished, interrupted: ops.interrupted, ops: ops)
            .onAppear { ops.refreshInterrupted(); ops.refreshOrder() }
    }
}

/// The popover's content for a given list of ops. With no `ops` model (the
/// power tour's demo) it only draws: the buttons do nothing.
struct OpsQueueContent: View {
    let active: [OpStatus]
    let finished: [OpStatus]
    let interrupted: [OpStatus]
    let ops: OpsModel?
    /// Each row's laid-out height, keyed by section and op (rows grow when
    /// their failed items are shown). Rows not drawn yet use an estimate.
    @State private var rowHeights: [String: CGFloat] = [:]

    /// The tallest the popover gets; past this the list scrolls.
    static let maxHeight: CGFloat = 560
    static let headerHeight: CGFloat = 42
    /// A section title's height in the plain list.
    static let sectionHeight: CGFloat = 29

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Text("Operations").font(.headline)
                Spacer()
                Button("Clear Finished") { ops?.clearFinished() }
                    .buttonStyle(.link).disabled(finished.isEmpty)
            }
            .padding(.horizontal, 14).padding(.vertical, 10)
            Divider()
            if active.isEmpty && finished.isEmpty && interrupted.isEmpty {
                VStack(spacing: 6) {
                    Image(systemName: "tray").font(.system(size: 26)).foregroundStyle(.tertiary)
                    Text("No operations").foregroundStyle(.secondary)
                }
                .frame(maxWidth: .infinity, minHeight: 110)
            } else {
                List {
                    if !interrupted.isEmpty {
                        Section("Interrupted — from the last time") {
                            ForEach(interrupted) { InterruptedRow(op: $0, ops: ops).modifier(RowHeight(key: "i\($0.id)", into: $rowHeights)) }
                        }
                    }
                    if !active.isEmpty {
                        Section("In progress") {
                            ForEach(active) { ActiveRow(op: $0, ops: ops).modifier(RowHeight(key: "a\($0.id)", into: $rowHeights)) }
                                .onMove { from, to in
                                    var ids = active.map(\.id)
                                    guard let f = from.first else { return }
                                    let moved = ids[f]
                                    ids.move(fromOffsets: from, toOffset: to)
                                    ops?.moved(moved, visible: ids)
                                }
                        }
                    }
                    if !finished.isEmpty {
                        Section("Finished") {
                            ForEach(finished) { FinishedRow(op: $0, ops: ops).modifier(RowHeight(key: "f\($0.id)", into: $rowHeights)) }
                        }
                    }
                }
                .listStyle(.plain)
                .scrollContentBackground(.hidden)
            }
        }
        .frame(width: 440, height: height)
    }

    /// Fits the ops, up to `maxHeight`.
    private var height: CGFloat {
        if active.isEmpty && finished.isEmpty && interrupted.isEmpty { return 150 }
        func sum(_ ops: [OpStatus], _ k: String, _ guess: CGFloat) -> CGFloat {
            ops.reduce(0) { $0 + (rowHeights["\(k)\($1.id)"] ?? guess) }
        }
        let rows = sum(interrupted, "i", 70) + sum(active, "a", 80) + sum(finished, "f", 46)
        let sections = CGFloat([interrupted, active, finished].filter { !$0.isEmpty }.count)
        return min(Self.maxHeight, max(150, Self.headerHeight + sections * Self.sectionHeight + rows + 8))
    }

}

/// Reports a list row's full height (its cell, padding included) through
/// the row background, which spans the whole cell.
private struct RowHeight: ViewModifier {
    let key: String
    @Binding var into: [String: CGFloat]
    func body(content: Content) -> some View {
        content.listRowBackground(Color.clear.onGeometryChange(for: CGFloat.self, of: { $0.size.height }) { h in
            if abs((into[key] ?? 0) - h) > 0.5 { into[key] = h }
        })
    }
}

private func kindSymbol(_ k: OpKind) -> String {
    switch k {
    case .copy, .duplicate: "doc.on.doc"
    case .move, .moveTo: "arrow.right.doc.on.clipboard"
    case .trash: "trash"
    case .deleteImmediately: "trash.slash"
    case .newFolder: "folder.badge.plus"
    case .newFile: "doc.badge.plus"
    case .rename: "pencil"
    case .compound: "square.stack.3d.down.right"
    }
}

private struct ActiveRow: View {
    let op: OpStatus
    let ops: OpsModel?

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: kindSymbol(op.kind)).foregroundStyle(.secondary).frame(width: 20).padding(.top, 2)
            VStack(alignment: .leading, spacing: 4) {
                Text(op.title).font(.system(size: 13, weight: .medium)).lineLimit(1).truncationMode(.middle)
                ProgressView(value: op.fraction)
                    .progressViewStyle(.linear)
                    .tint(op.state == .paused ? .gray : (op.state == .waitingForDecision ? .orange : .accentColor))
                HStack(spacing: 4) {
                    Text(counts).monospacedDigit()
                    Spacer(minLength: 4)
                    Text(rate).monospacedDigit()
                }
                .font(.caption).foregroundStyle(.secondary)
                if let c = op.current, op.state == .running || op.state == .paused {
                    Text((c as NSString).lastPathComponent).font(.caption2).foregroundStyle(.tertiary)
                        .lineLimit(1).truncationMode(.middle)
                }
                ErrorsDisclosure(errors: ops?.errors(of: op) ?? op.errors)
            }
            VStack(spacing: 6) {
                if op.state == .paused {
                    Button { ops?.resume(op) } label: { Image(systemName: "play.fill") }.help("Resume")
                } else if op.state != .waitingForDecision {
                    Button { ops?.pause(op) } label: { Image(systemName: "pause.fill") }.help("Pause")
                }
                Button { ops?.cancel(op) } label: { Image(systemName: "xmark") }.help("Cancel")
            }
            .buttonStyle(.borderless).controlSize(.small)
        }
        .padding(.vertical, 4)
    }

    private var counts: String {
        var t = op.bytesTotal > 0 ? "\(OpsFormat.bytes(op.bytesDone)) of \(OpsFormat.bytes(op.bytesTotal))" : ""
        if op.filesTotal > 1 {
            t += (t.isEmpty ? "" : " · ") + "\(op.filesDone.formatted()) of \(op.filesTotal.formatted()) files"
        }
        return t.isEmpty ? OpsFormat.stateText(op.state) : t
    }

    private var rate: String {
        switch op.state {
        case .running: "\(OpsFormat.speed(op.speedBps)) · \(OpsFormat.eta(op.etaSecs))"
        default: OpsFormat.stateText(op.state)
        }
    }
}

private struct FinishedRow: View {
    let op: OpStatus
    let ops: OpsModel?

    var body: some View {
        let errors = ops?.errors(of: op) ?? op.errors
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: symbol).foregroundStyle(color).frame(width: 20).padding(.top, 1)
            VStack(alignment: .leading, spacing: 3) {
                Text(op.title).font(.system(size: 13)).lineLimit(1).truncationMode(.middle)
                Text(summary(errors.count)).font(.caption).foregroundStyle(.secondary)
                ErrorsDisclosure(errors: errors)
            }
            Spacer(minLength: 0)
            if op.current != nil || !errors.isEmpty {
                Button("Show in Folder") { ops?.reveal(op) }.buttonStyle(.link).font(.caption)
            }
        }
        .padding(.vertical, 3)
    }

    private var symbol: String {
        switch op.state {
        case .completed: "checkmark.circle.fill"
        case .cancelled: "slash.circle"
        default: "exclamationmark.triangle.fill"
        }
    }
    private var color: Color {
        switch op.state {
        case .completed: .green
        case .cancelled: .secondary
        default: .orange
        }
    }
    /// The core's line ("Failed · 0 of 1 done · 1 failed"), plus the size.
    private func summary(_ failed: Int) -> String {
        var parts = [op.summary.isEmpty ? OpsFormat.stateText(op.state) : op.summary]
        if op.bytesTotal > 0 { parts.append(OpsFormat.bytes(op.bytesTotal)) }
        return parts.joined(separator: " · ")
    }
}

private struct InterruptedRow: View {
    let op: OpStatus
    let ops: OpsModel?

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: "exclamationmark.arrow.triangle.2.circlepath").foregroundStyle(.blue).frame(width: 20)
            VStack(alignment: .leading, spacing: 3) {
                Text(op.title).font(.system(size: 13)).lineLimit(1).truncationMode(.middle)
                ProgressView(value: op.fraction).tint(.blue)
                Text("Stopped at \(Int((op.fraction * 100).rounded())) % · \(op.filesDone.formatted()) of \(op.filesTotal.formatted()) files")
                    .font(.caption).foregroundStyle(.secondary)
                // What it needs, worded by the core ("Insert “CLIENT_A” to resume").
                if let note = op.resumeNote {
                    Text(note).font(.caption).foregroundStyle(op.canResume ? Color.secondary : Color.orange)
                }
            }
            VStack(spacing: 4) {
                Button("Resume Transfer") { ops?.resume(op) }.buttonStyle(.borderedProminent)
                    .disabled(!op.canResume)
                Button("Cancel") { ops?.cancel(op) }.buttonStyle(.bordered)
            }
            .controlSize(.small)
        }
        .padding(.vertical, 4)
    }
}

/// "3 items failed" that opens to one line per file.
private struct ErrorsDisclosure: View {
    let errors: [ItemError]
    #if DEBUG
    @State private var open = UserDefaults.standard.bool(forKey: "FSExpandErrors")
    #else
    @State private var open = false
    #endif

    var body: some View {
        if !errors.isEmpty {
            VStack(alignment: .leading, spacing: 3) {
                Button {
                    open.toggle()
                } label: {
                    HStack(spacing: 4) {
                        Image(systemName: "chevron.right").font(.system(size: 9, weight: .bold))
                            .rotationEffect(.degrees(open ? 90 : 0))
                        Label("\(errors.count) \(errors.count == 1 ? "item" : "items") failed",
                              systemImage: "exclamationmark.triangle.fill")
                    }
                    .font(.caption).foregroundStyle(.red)
                }
                .buttonStyle(.plain)
                if open {
                    ForEach(Array(errors.prefix(50).enumerated()), id: \.offset) { _, e in
                        VStack(alignment: .leading, spacing: 0) {
                            Text((e.path as NSString).lastPathComponent).font(.caption.weight(.medium)).lineLimit(1)
                            Text(e.message).font(.caption2).foregroundStyle(.secondary)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                        .padding(.leading, 14)
                    }
                    if errors.count > 50 {
                        Text("and \(errors.count - 50) more").font(.caption2).foregroundStyle(.secondary).padding(.leading, 14)
                    }
                }
            }
        }
    }
}

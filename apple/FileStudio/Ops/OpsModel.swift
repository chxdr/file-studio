// OpsModel.swift — what the ops UI needs on top of `AppModel.ops`: queue
// order, per-file errors, interrupted ops from an earlier run, pending
// conflicts, and the text and numbers the ring, popover and status line show.
// It decides nothing about files; every button calls the engine.

import AppKit
import Observation

@MainActor
@Observable
final class OpsModel {
    private unowned let app: AppModel
    /// Left over from an earlier run (resume or cancel them).
    private(set) var interrupted: [OpStatus] = []
    /// Collisions waiting for an answer, oldest first.
    private(set) var pending: [Conflict] = []
    /// How many conflicts the current run of the sheet has covered / will cover.
    private(set) var conflictsAnswered = 0
    private(set) var conflictsTotal = 0
    /// Queue order (ids), from the core.
    private(set) var order: [OpID] = []
    /// Per-file errors reported by `.opFailed`, by op.
    private(set) var failures: [OpID: [ItemError]] = [:]
    /// Open the queue popover (the toolbar button binds to this).
    var popoverShown = false
    @ObservationIgnored private var token: ObserverToken?

    static func of(_ m: AppModel) -> OpsModel { m.attachment(OpsModel.self) { OpsModel(m) } }

    private init(_ m: AppModel) {
        app = m
        refreshOrder()
        refreshInterrupted()
        token = m.observe { [weak self] e in self?.event(e) }
    }

    // MARK: Events

    private func event(_ e: CoreEvent) {
        switch e {
        case .opConflict(_, let c):
            if !pending.contains(where: { $0.op == c.op && $0.id == c.id }) {
                pending.append(c)
                conflictsTotal += 1
            }
        case .opFailed(let op, let path, let msg):
            var l = failures[op] ?? []
            if !l.contains(where: { $0.path == path && $0.message == msg }) { l.append(ItemError(path: path, message: msg)) }
            failures[op] = l
        case .opProgress(let list):
            if list.contains(where: { !order.contains($0.id) }) { refreshOrder() }
            // A conflict the core no longer lists was answered elsewhere.
            for s in list where s.state != .waitingForDecision {
                pending.removeAll { p in p.op == s.id && !s.conflicts.contains(where: { $0.id == p.id }) }
            }
            if pending.isEmpty { conflictsAnswered = 0; conflictsTotal = 0 }
        case .opFinished(let op, _):
            pending.removeAll { $0.op == op }
        default: break
        }
    }

    func refreshOrder() { order = app.engine.opsList().map(\.id) }

    func refreshInterrupted() { interrupted = (try? app.engine.interruptedOps()) ?? [] }

    // MARK: Derived

    /// This session's ops in queue order.
    var ops: [OpStatus] {
        app.ops.sorted { (order.firstIndex(of: $0.id) ?? .max) < (order.firstIndex(of: $1.id) ?? .max) }
    }
    var active: [OpStatus] { ops.filter { !$0.state.isFinished } }
    var finished: [OpStatus] { ops.filter(\.state.isFinished) }

    /// 0…1 over every unfinished op, by bytes when known.
    var overallFraction: Double {
        let a = active
        guard !a.isEmpty else { return 0 }
        let bt = a.reduce(0) { $0 + $1.bytesTotal }
        if bt > 0 { return min(1, Double(a.reduce(0) { $0 + $1.bytesDone }) / Double(bt)) }
        let ft = a.reduce(0) { $0 + $1.filesTotal }
        return ft > 0 ? min(1, Double(a.reduce(0) { $0 + $1.filesDone }) / Double(ft)) : 0
    }

    /// Ops with at least one failed item.
    var failedCount: Int { ops.filter { errors(of: $0).count > 0 || $0.state == .failed }.count }
    var conflictCount: Int { pending.count + active.filter { $0.state == .waitingForDecision && $0.conflicts.isEmpty }.count }
    var needsAttention: Bool { !interrupted.isEmpty }

    func errors(of op: OpStatus) -> [ItemError] {
        var l = op.errors
        for f in failures[op.id] ?? [] where !l.contains(where: { $0.path == f.path && $0.message == f.message }) { l.append(f) }
        return l
    }

    // MARK: Actions

    func pause(_ op: OpStatus) { run { try app.engine.pauseOp(op.id) } }
    func resume(_ op: OpStatus) { run { try app.engine.resumeOp(op.id) }; refreshInterrupted() }
    func cancel(_ op: OpStatus) { run { try app.engine.cancelOp(op.id) }; refreshInterrupted() }

    func clearFinished() {
        app.engine.clearFinishedOps()
        app.removeFinishedOps()
        failures = failures.filter { id, _ in app.ops.contains { $0.id == id } }
        refreshOrder()
    }

    /// Dragging in the popover: `ids` is the visible order after the move.
    func moved(_ id: OpID, visible ids: [OpID]) {
        let full = app.engine.opsList().map(\.id).filter { $0 != id }
        guard let i = ids.firstIndex(of: id) else { return }
        let target: Int
        if i + 1 < ids.count, let n = full.firstIndex(of: ids[i + 1]) { target = n } else { target = full.count }
        run { try app.engine.reorderOp(id, to: UInt32(target)) }
        refreshOrder()
    }

    func resolve(_ c: Conflict, _ decision: ConflictDecision, applyToAll: Bool) {
        run { try app.engine.resolveConflict(op: c.op, conflict: c.id, decision: decision, applyToAll: applyToAll) }
        pending.removeAll { $0.op == c.op && $0.id == c.id }
        conflictsAnswered += 1
        if applyToAll, let remaining = app.engine.opStatus(c.op)?.conflicts {
            let ids = Set(remaining.map(\.id))
            let before = pending.count
            pending.removeAll { $0.op == c.op && !ids.contains($0.id) }
            conflictsAnswered += before - pending.count
        }
        if pending.isEmpty { conflictsAnswered = 0; conflictsTotal = 0 }
    }

    func reveal(_ op: OpStatus) {
        if let p = op.current ?? errors(of: op).first?.path { PlatformActions.reveal([p]) }
    }

    private func run(_ f: () throws -> Void) {
        do { try f() } catch { ToastCenter.of(app).show(error.localizedDescription, style: .error) }
    }

    // MARK: Text

    /// "Copying" for the status line.
    static func verb(_ k: OpKind) -> String {
        switch k {
        case .copy: "Copying"
        case .move, .moveTo: "Moving"
        case .trash: "Moving to Trash"
        case .deleteImmediately: "Deleting"
        case .newFolder, .newFile: "Creating"
        case .rename: "Renaming"
        case .duplicate: "Duplicating"
        case .compound: "Working"
        }
    }

    /// "Copying… 42 %", or nil when nothing is running.
    var statusText: String? {
        let a = active
        guard let first = a.first(where: { $0.state == .running }) ?? a.first else { return nil }
        switch first.state {
        case .paused: return "\(Self.verb(first.kind)) paused"
        case .waitingForDecision: return "\(first.title): waiting for you"
        default:
            let more = a.count > 1 ? " (+\(a.count - 1))" : ""
            return "\(Self.verb(first.kind))… \(Int((first.fraction * 100).rounded())) %\(more)"
        }
    }
}

// MARK: - Formatting

enum OpsFormat {
    static func bytes(_ n: UInt64) -> String {
        let f = ByteCountFormatter()
        f.countStyle = .file
        f.allowsNonnumericFormatting = false   // "0 bytes", not "Zero KB"
        return f.string(fromByteCount: Int64(n))
    }
    static func speed(_ bps: Double) -> String { bps > 0 ? bytes(UInt64(bps)) + "/s" : "—" }
    static func eta(_ s: Double?) -> String {
        guard let s, s.isFinite, s >= 0 else { return "—" }
        let t = Int(s.rounded())
        if t < 60 { return "\(max(t, 1)) s left" }
        if t < 3600 { return "\(t / 60) min \(t % 60) s left" }
        return "\(t / 3600) h \((t % 3600) / 60) min left"
    }
    static func stateText(_ s: OpState) -> String {
        switch s {
        case .queued: "Waiting"
        case .running: "Running"
        case .paused: "Paused"
        case .waitingForDecision: "Needs your decision"
        case .completed: "Done"
        case .completedWithErrors: "Done, with errors"
        case .cancelled: "Cancelled"
        case .failed: "Failed"
        case .interrupted: "Interrupted"
        }
    }
}

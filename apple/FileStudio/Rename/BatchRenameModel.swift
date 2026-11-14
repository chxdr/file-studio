// BatchRenameModel.swift — the state behind one "Batch Rename N Items…"
// sheet. It holds the steps being edited and the core's summary; every edit
// goes straight to the core (`setSteps`), which works out every new name and
// problem. No rename rules live here.

import AppKit
import Observation

@MainActor
@Observable
final class BatchRenameModel {
    let session: RenameSessionAPI
    let engine: RenameEngineAPI
    let itemCount: Int
    let tokens: [RenameToken]
    var steps: [RenameStep] { didSet { push() } }
    private(set) var summary = RenameSummary()
    private(set) var presets: [RenamePreset] = []
    /// A message under the steps (apply refused, preset error).
    var notice: String?
    /// The step whose first field should take focus (a new or loaded step).
    var focusStep: UUID?
    @ObservationIgnored private var poll: Timer?
    @ObservationIgnored private var pushing = false

    init(session: RenameSessionAPI, engine: RenameEngineAPI, itemCount: Int) {
        self.session = session
        self.engine = engine
        self.itemCount = itemCount
        tokens = engine.renameTokens()
        let s = session.steps()
        steps = s.isEmpty ? [RenameStep(kind: .template)] : s
        summary = session.summary()
        presets = engine.renamePresets()
        focusStep = steps.first?.id
        push()
    }

    /// Sends the steps to the core and takes its summary.
    private func push() {
        guard !pushing else { return }
        pushing = true
        defer { pushing = false }
        summary = session.setSteps(steps)
        notice = nil
        watchExif()
    }

    /// While EXIF is read in the background, refresh the summary (and rows).
    private func watchExif() {
        guard summary.exifLoading, poll == nil else { return }
        poll = Timer.scheduledTimer(withTimeInterval: 0.2, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self else { return }
                self.summary = self.session.summary()
                if !self.summary.exifLoading { self.stopPolling() }
            }
        }
    }

    func stopPolling() {
        poll?.invalidate()
        poll = nil
    }

    // MARK: Steps

    func add(_ kind: RenameStepKind) {
        let s = RenameStep(kind: kind)
        steps.append(s)
        focusStep = s.id
    }

    func remove(_ id: UUID) {
        steps.removeAll { $0.id == id }
        if steps.isEmpty { steps = [RenameStep(kind: .template)] }
    }

    func move(_ id: UUID, by delta: Int) {
        guard let i = steps.firstIndex(where: { $0.id == id }) else { return }
        let j = i + delta
        guard steps.indices.contains(j) else { return }
        steps.swapAt(i, j)
    }

    // MARK: Presets

    func load(_ p: RenamePreset) {
        steps = p.steps.isEmpty ? [RenameStep(kind: .template)] : p.steps
        focusStep = steps.first?.id
    }

    func savePreset(named name: String) {
        do {
            try engine.saveRenamePreset(name: name, steps: steps)
            presets = engine.renamePresets()
        } catch {
            notice = error.localizedDescription
        }
    }

    func deletePreset(_ name: String) {
        do {
            try engine.deleteRenamePreset(name: name)
            presets = engine.renamePresets()
        } catch {
            notice = error.localizedDescription
        }
    }

    // MARK: Footer

    var canApply: Bool { summary.error == nil && !summary.exifLoading && summary.ready > 0 }

    var applyTitle: String {
        summary.ready == 1 ? "Rename 1 Item" : "Rename \(summary.ready) Items"
    }

    /// "10 will be renamed · 2 unchanged · 1 problem".
    var status: String {
        var parts = ["\(summary.ready) will be renamed"]
        if summary.unchanged > 0 { parts.append("\(summary.unchanged) unchanged") }
        if summary.problems > 0 { parts.append(summary.problems == 1 ? "1 problem" : "\(summary.problems) problems") }
        if summary.warnings > 0 { parts.append(summary.warnings == 1 ? "1 warning" : "\(summary.warnings) warnings") }
        return parts.joined(separator: " · ")
    }

    /// Renames the ready rows through the core. Returns the outcome to hand
    /// to the app, or nil (and a notice) if the core refused.
    func apply() -> Outcome? {
        do {
            let out = try session.apply()
            stopPolling()
            return out
        } catch {
            notice = error.localizedDescription
            return nil
        }
    }
}

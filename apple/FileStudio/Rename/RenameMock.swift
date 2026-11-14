// RenameMock.swift — RenameAPI for MockCore (`-FSMockCore`), so the sheet
// can be drawn without Rust. It previews every name as unchanged and never
// renames anything; the real rules are in fstudio-rename.

import Foundation

final class MockRenameSession: RenameSessionAPI {
    private var names: [String]
    private var current: [RenameStep] = [RenameStep(kind: .template)]
    private var version: UInt64 = 1

    init(paths: [String]) { names = paths.map { ($0 as NSString).lastPathComponent } }

    func steps() -> [RenameStep] { current }
    func setSteps(_ steps: [RenameStep]) -> RenameSummary {
        current = steps
        version += 1
        return summary()
    }
    func summary() -> RenameSummary {
        RenameSummary(version: version, total: UInt32(names.count), unchanged: UInt32(names.count))
    }
    func rows(start: UInt32, count: UInt32) -> [RenameRow] {
        let s = Int(start), e = min(names.count, s + Int(count))
        guard s < e else { return [] }
        return (s..<e).map {
            RenameRow(index: UInt32($0), oldName: names[$0], newName: names[$0], status: .unchanged,
                      message: "", warning: "", folder: "")
        }
    }
    func apply() throws -> Outcome { throw CoreError(.notAvailable, "The mock core doesn't rename") }
}

extension MockEngine: RenameEngineAPI {
    func batchRename(tab: TabID?, paths: [String]) throws -> RenameSessionAPI {
        guard !paths.isEmpty else { throw CoreError(.notAvailable, "nothing is selected") }
        return MockRenameSession(paths: paths)
    }
    func renamePresets() -> [RenamePreset] { [] }
    func saveRenamePreset(name: String, steps: [RenameStep]) throws {}
    func deleteRenamePreset(name: String) throws -> Bool { false }
    func renameTokens() -> [RenameToken] { [RenameToken(token: "{name}", title: "Name")] }
}

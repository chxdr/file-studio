// RenameAPI.swift — the batch-rename part of the core contract (M3).
//
// Same rules as Core/CoreAPI.swift: the core works out every new name, every
// problem and the order of the renames; the sheet only edits steps and draws
// rows. `RustEngine` implements this in RenameRust.swift, `MockEngine` in
// RenameMock.swift.

import Foundation

/// Which step a `RenameStep` is. Only the fields its kind names are used.
public enum RenameStepKind: String, CaseIterable, Equatable, Sendable, Identifiable {
    case template, replace, changeCase, sequence, trim, insert, ext
    public var id: String { rawValue }
    public var title: String {
        switch self {
        case .template: "Name Template"
        case .replace: "Find & Replace"
        case .changeCase: "Change Case"
        case .sequence: "Sequence Number"
        case .trim: "Trim"
        case .insert: "Insert Text"
        case .ext: "Extension"
        }
    }
    public var symbol: String {
        switch self {
        case .template: "curlybraces"
        case .replace: "magnifyingglass"
        case .changeCase: "textformat"
        case .sequence: "number"
        case .trim: "scissors"
        case .insert: "text.insert"
        case .ext: "doc.badge.gearshape"
        }
    }
}

public enum RenameTarget: String, CaseIterable, Equatable, Sendable {
    case name, ext, whole
    public var title: String {
        switch self {
        case .name: "Name"
        case .ext: "Extension"
        case .whole: "Name and Extension"
        }
    }
}

public enum RenameCase: String, CaseIterable, Equatable, Sendable {
    case lower, upper, title, sentence
    public var title: String {
        switch self {
        case .lower: "lower case"
        case .upper: "UPPER CASE"
        case .title: "Title Case"
        case .sentence: "Sentence case"
        }
    }
}

public enum RenamePosition: String, CaseIterable, Equatable, Sendable {
    case start, end, index
    public var title: String {
        switch self {
        case .start: "At the Start"
        case .end: "At the End"
        case .index: "After Character…"
        }
    }
}

public enum RenameTrimMode: String, CaseIterable, Equatable, Sendable {
    case whitespace, start, end, characters
    public var title: String {
        switch self {
        case .whitespace: "Extra Spaces"
        case .start: "Characters from the Start"
        case .end: "Characters from the End"
        case .characters: "These Characters at Either End"
        }
    }
}

public enum RenameExtChange: String, CaseIterable, Equatable, Sendable {
    case lower, upper, set, remove
    public var title: String {
        switch self {
        case .lower: "lower case"
        case .upper: "UPPER CASE"
        case .set: "Change To…"
        case .remove: "Remove"
        }
    }
}

/// One step of the pipeline (the core's flat `FfiRenameStep`).
public struct RenameStep: Equatable, Sendable, Identifiable {
    /// For SwiftUI lists only; never crosses to the core.
    public var id = UUID()
    public var kind: RenameStepKind
    public var enabled = true
    public var template = "{name}"
    public var find = ""
    public var replace = ""
    public var regex = false
    public var ignoreCase = false
    public var firstOnly = false
    public var target: RenameTarget = .name
    public var caseKind: RenameCase = .title
    public var start: Int64 = 1
    public var step: Int64 = 1
    public var padding: UInt32 = 3
    public var position: RenamePosition = .start
    public var index: UInt32 = 0
    public var separator = " "
    public var text = ""
    public var trimMode: RenameTrimMode = .whitespace
    public var count: UInt32 = 1
    public var chars = ""
    public var extChange: RenameExtChange = .lower
    public var ext = ""

    public init(kind: RenameStepKind) {
        self.kind = kind
        if kind == .sequence { position = .end }
    }

    /// Equal ignoring `id` (what the core would see).
    public func sameSettings(as o: RenameStep) -> Bool {
        var a = self
        a.id = o.id
        return a == o
    }
}

public enum RenameRowStatus: Equatable, Sendable { case ready, unchanged, problem }

/// One preview row.
public struct RenameRow: Equatable, Sendable {
    public var index: UInt32
    public var oldName: String
    public var newName: String
    public var status: RenameRowStatus
    /// The problem, in words, or empty.
    public var message: String
    /// A warning that doesn't stop the rename, or empty.
    public var warning: String
    /// The item's folder when the batch spans folders, else empty.
    public var folder: String
}

/// The preview's counts and state.
public struct RenameSummary: Equatable, Sendable {
    /// Reload the visible rows when this changes.
    public var version: UInt64 = 0
    public var total: UInt32 = 0
    public var ready: UInt32 = 0
    public var unchanged: UInt32 = 0
    public var problems: UInt32 = 0
    public var warnings: UInt32 = 0
    public var exifLoading = false
    public var exifDone: UInt32 = 0
    public var exifTotal: UInt32 = 0
    /// Why the steps can't run, if they can't.
    public var error: String?
    public var errorStep: UInt32?
    public var manyFolders = false
}

public struct RenamePreset: Equatable, Sendable {
    public var name: String
    public var steps: [RenameStep]
}

public struct RenameToken: Equatable, Sendable {
    public var token: String
    public var title: String
}

/// One open "Batch Rename" sheet. Every call is quick and never touches the disk.
public protocol RenameSessionAPI: AnyObject {
    func steps() -> [RenameStep]
    /// Replaces the steps and recomputes the preview (call on every edit).
    func setSteps(_ steps: [RenameStep]) -> RenameSummary
    /// The counts and state (poll while `exifLoading`).
    func summary() -> RenameSummary
    /// Rows `start..<start+count`, for the visible part of the table only.
    func rows(start: UInt32, count: UInt32) -> [RenameRow]
    /// Renames every ready row as one undoable operation.
    func apply() throws -> Outcome
}

/// Engine calls for batch rename.
public protocol RenameEngineAPI: AnyObject {
    /// Opens a session for `paths` (list order). Lists each folder once.
    func batchRename(tab: TabID?, paths: [String]) throws -> RenameSessionAPI
    func renamePresets() -> [RenamePreset]
    func saveRenamePreset(name: String, steps: [RenameStep]) throws
    @discardableResult func deleteRenamePreset(name: String) throws -> Bool
    /// The template tokens, for the Insert Token menu.
    func renameTokens() -> [RenameToken]
}

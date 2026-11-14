// RenameRust.swift — RenameAPI over the UniFFI bindings. Pure translation.

import Foundation

private func renameWrap<T>(_ body: () throws -> T) throws -> T {
    do { return try body() } catch let e as FfiError {
        switch e {
        case .NotFound(let m): throw CoreError(.notFound, m)
        case .NotAvailable(let m): throw CoreError(.notAvailable, m)
        case .InvalidName(let m): throw CoreError(.invalidName, m)
        case .InvalidSetting(let m): throw CoreError(.invalidSetting, m)
        case .UnknownCommand(let m): throw CoreError(.unknownCommand, m)
        case .Failed(let m): throw CoreError(.failed, m)
        }
    }
}

extension RenameStep {
    init(_ f: FfiRenameStep) {
        self.init(kind: {
            switch f.kind {
            case .template: .template
            case .replace: .replace
            case .case: .changeCase
            case .sequence: .sequence
            case .trim: .trim
            case .insert: .insert
            case .extension: .ext
            }
        }())
        enabled = f.enabled
        template = f.template
        find = f.find
        replace = f.replace
        regex = f.regex
        ignoreCase = f.ignoreCase
        firstOnly = f.firstOnly
        target = switch f.target { case .name: .name; case .extension: .ext; case .whole: .whole }
        caseKind = switch f.caseKind { case .lower: .lower; case .upper: .upper; case .title: .title; case .sentence: .sentence }
        start = f.start
        step = f.step
        padding = f.padding
        position = switch f.position { case .start: .start; case .end: .end; case .index: .index }
        index = f.index
        separator = f.separator
        text = f.text
        trimMode = switch f.trimMode { case .whitespace: .whitespace; case .start: .start; case .end: .end; case .characters: .characters }
        count = f.count
        chars = f.chars
        extChange = switch f.extChange { case .lower: .lower; case .upper: .upper; case .set: .set; case .remove: .remove }
        ext = f.ext
    }

    var ffi: FfiRenameStep {
        FfiRenameStep(
            kind: kind.ffi, enabled: enabled, template: template, find: find, replace: replace, regex: regex,
            ignoreCase: ignoreCase, firstOnly: firstOnly, target: target.ffi, caseKind: caseKind.ffi,
            start: start, step: step, padding: padding, position: position.ffi, index: index,
            separator: separator, text: text, trimMode: trimMode.ffi, count: count, chars: chars,
            extChange: extChange.ffi, ext: ext)
    }
}

private extension RenameStepKind {
    var ffi: FfiRenameStepKind {
        switch self {
        case .template: .template
        case .replace: .replace
        case .changeCase: .case
        case .sequence: .sequence
        case .trim: .trim
        case .insert: .insert
        case .ext: .extension
        }
    }
}

private extension RenameTarget {
    var ffi: FfiRenameTarget {
        switch self {
        case .name: .name
        case .ext: .extension
        case .whole: .whole
        }
    }
}

private extension RenameCase {
    var ffi: FfiCaseKind {
        switch self {
        case .lower: .lower
        case .upper: .upper
        case .title: .title
        case .sentence: .sentence
        }
    }
}

private extension RenamePosition {
    var ffi: FfiRenamePosition {
        switch self {
        case .start: .start
        case .end: .end
        case .index: .index
        }
    }
}

private extension RenameTrimMode {
    var ffi: FfiTrimMode {
        switch self {
        case .whitespace: .whitespace
        case .start: .start
        case .end: .end
        case .characters: .characters
        }
    }
}

private extension RenameExtChange {
    var ffi: FfiExtChange {
        switch self {
        case .lower: .lower
        case .upper: .upper
        case .set: .set
        case .remove: .remove
        }
    }
}

extension RenameSummary {
    init(_ f: FfiRenameSummary) {
        self.init(version: f.version, total: f.total, ready: f.ready, unchanged: f.unchanged,
                  problems: f.problems, warnings: f.warnings, exifLoading: f.exifLoading,
                  exifDone: f.exifDone, exifTotal: f.exifTotal, error: f.error,
                  errorStep: f.errorStep, manyFolders: f.manyFolders)
    }
}

extension RenameRow {
    init(_ f: FfiRenameRow) {
        let status: RenameRowStatus = switch f.status {
        case .ready: .ready
        case .unchanged: .unchanged
        case .problem: .problem
        }
        self.init(index: f.index, oldName: f.oldName, newName: f.newName, status: status,
                  message: f.message, warning: f.warning, folder: f.folder)
    }
}

final class RustRenameSession: RenameSessionAPI {
    private let ffi: FfiRenameSession
    init(_ ffi: FfiRenameSession) { self.ffi = ffi }

    func steps() -> [RenameStep] { ffi.steps().map(RenameStep.init) }
    func setSteps(_ steps: [RenameStep]) -> RenameSummary { RenameSummary(ffi.setSteps(steps: steps.map(\.ffi))) }
    func summary() -> RenameSummary { RenameSummary(ffi.summary()) }
    func rows(start: UInt32, count: UInt32) -> [RenameRow] { ffi.rows(start: start, count: count).map(RenameRow.init) }
    func apply() throws -> Outcome { try renameWrap { Outcome(try ffi.apply()) } }
}

extension RustEngine: RenameEngineAPI {
    func batchRename(tab: TabID?, paths: [String]) throws -> RenameSessionAPI {
        try renameWrap { RustRenameSession(try ffi.batchRename(tab: tab, paths: paths)) }
    }
    func renamePresets() -> [RenamePreset] {
        ffi.renamePresets().map { RenamePreset(name: $0.name, steps: $0.steps.map(RenameStep.init)) }
    }
    func saveRenamePreset(name: String, steps: [RenameStep]) throws {
        try renameWrap { try ffi.saveRenamePreset(name: name, steps: steps.map(\.ffi)) }
    }
    func deleteRenamePreset(name: String) throws -> Bool {
        try renameWrap { try ffi.deleteRenamePreset(name: name) }
    }
    func renameTokens() -> [RenameToken] {
        FileStudio.renameTokens().map { RenameToken(token: $0.token, title: $0.title) }
    }
}

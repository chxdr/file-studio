// BatchRenameSheet.swift — shows the "Batch Rename N Items…" sheet when the
// core asks for it (`ShellRequest.batchRename`, from ⌃⌘R, the File menu or
// the multi-selection context menu), and hands the outcome back to the app.

import AppKit
import SwiftUI

@MainActor
enum BatchRenameSheet {
    private static var sheet: NSWindow?
    private static weak var host: NSWindow?
    private(set) static var model: BatchRenameModel?
    private static var window: WindowID?

    /// Registers for the core's `batchRename` request.
    static func install(_ app: AppModel) {
        app.registerShellHandler(for: "batchRename") { req, ctx in
            guard case .batchRename(let tab, let paths) = req else { return false }
            show(app: app, tab: tab, paths: paths, window: ctx.window)
            return true
        }
        #if DEBUG
        BatchRenameDebugSteps.install()
        #endif
    }

    private static func hostWindow(_ id: WindowID?) -> NSWindow? {
        let mains = NSApp.windows.filter { $0.windowController is CoreWindowIdentifying && $0.isVisible }
        if let id, let w = mains.first(where: { ($0.windowController as? CoreWindowIdentifying)?.coreWindowID == id }) {
            return w
        }
        return NSApp.keyWindow.flatMap { k in mains.first { $0 === k || $0 === k.sheetParent } } ?? mains.first
    }

    static func show(app: AppModel, tab: TabID, paths: [String], window: WindowID?) {
        guard sheet == nil, let host = hostWindow(window) else { return }
        guard let engine = app.engine as? RenameEngineAPI else { return }
        let session: RenameSessionAPI
        do {
            session = try engine.batchRename(tab: tab, paths: paths)
        } catch {
            app.handle(Outcome(message: error.localizedDescription), in: window)
            return
        }
        let m = BatchRenameModel(session: session, engine: engine, itemCount: paths.count)
        model = m
        self.window = window
        let view = BatchRenameView(
            model: m,
            cancel: { close() },
            rename: { apply(app) },
            savePreset: { askPresetName() })
        let vc = NSHostingController(rootView: view)
        let w = NSWindow(contentViewController: vc)
        w.styleMask = [.titled, .resizable]
        w.title = "Batch Rename"
        w.setContentSize(NSSize(width: 820, height: 640))
        w.contentMinSize = NSSize(width: 720, height: 560)
        sheet = w
        self.host = host
        host.beginSheet(w) { _ in }
    }

    /// The Rename button: queue the renames, close, and let the app show
    /// the outcome (the toast with Undo comes when the op finishes).
    static func apply(_ app: AppModel) {
        guard let m = model, m.canApply else { return }
        guard let out = m.apply() else { return }
        let win = window
        close()
        app.handle(out, in: win)
    }

    /// "Save as Preset…": asks for a name over the sheet.
    static func askPresetName() {
        guard let m = model, let s = sheet else { return }
        let a = NSAlert()
        a.messageText = "Save Preset"
        a.informativeText = "Saves these steps so you can use them again from the Presets menu."
        let field = NSTextField(frame: NSRect(x: 0, y: 0, width: 260, height: 24))
        field.placeholderString = "Preset name"
        a.accessoryView = field
        a.addButton(withTitle: "Save")
        a.addButton(withTitle: "Cancel")
        a.window.initialFirstResponder = field
        a.beginSheetModal(for: s) { resp in
            guard resp == .alertFirstButtonReturn else { return }
            let name = field.stringValue.trimmingCharacters(in: .whitespaces)
            MainActor.assumeIsolated { if !name.isEmpty { m.savePreset(named: name) } }
        }
    }

    static func close() {
        model?.stopPolling()
        if let s = sheet { host?.endSheet(s) ?? s.close() }
        sheet = nil
        model = nil
    }

    static var isShowing: Bool { sheet != nil }
}

#if DEBUG
/// Debug-automation steps for the sheet (`scripts/run-debug.sh`):
///
///   brename:show                    BRSUM + BRSTEP lines: the open sheet's summary and steps
///   brename:rows[:<n>]              BRROW lines: the first n preview rows (default 20)
///   brename:add:<kind>              add a step (template, replace, changeCase, sequence, trim, insert, ext)
///   brename:set:<i>:<field>=<value> set a field of step i (template, find, replace, regex, ignoreCase,
///                                   firstOnly, target, caseKind, start, step, padding, position, index,
///                                   separator, text, trimMode, count, chars, extChange, ext, enabled)
///   brename:remove:<i>              remove step i
///   brename:save:<name>             save the steps as a preset
///   brename:load:<name>             load a preset
///   brename:presets                 BRPRESET lines
///   brename:apply                   press Rename
///   brename:cancel                  press Cancel
@MainActor
enum BatchRenameDebugSteps {
    private static var installed = false

    static func install() {
        guard !installed else { return }
        installed = true
        DebugAutomation.register("brename") { arg, app in run(arg, app) }
    }

    private static func run(_ arg: String, _ app: AppModel) {
        let parts = arg.split(separator: ":", maxSplits: 1).map(String.init)
        let op = parts.first ?? ""
        let rest = parts.count > 1 ? parts[1] : ""
        guard let m = BatchRenameSheet.model else { print("BRENAME no sheet"); return }
        switch op {
        case "show":
            let s = m.summary
            print("BRSUM total=\(s.total) ready=\(s.ready) unchanged=\(s.unchanged) problems=\(s.problems) "
                  + "warnings=\(s.warnings) exif=\(s.exifLoading ? "loading" : "idle") error=\(s.error ?? "-") "
                  + "button=\"\(m.applyTitle)\" enabled=\(m.canApply) notice=\(m.notice ?? "-")")
            for (i, st) in m.steps.enumerated() {
                print("BRSTEP \(i) \(st.kind.rawValue) enabled=\(st.enabled) template=\(st.template) find=\(st.find) "
                      + "replace=\(st.replace) case=\(st.caseKind.rawValue) target=\(st.target.rawValue)")
            }
        case "rows":
            let n = UInt32(rest) ?? 20
            for r in m.session.rows(start: 0, count: n) {
                print("BRROW \(r.index) \(r.status) \(r.oldName) -> \(r.newName)"
                      + (r.message.isEmpty ? "" : " [\(r.message)]") + (r.warning.isEmpty ? "" : " {\(r.warning)}"))
            }
        case "add":
            if let k = RenameStepKind(rawValue: rest) { m.add(k) } else { print("BRENAME unknown kind \(rest)") }
        case "remove":
            if let i = Int(rest), m.steps.indices.contains(i) { m.remove(m.steps[i].id) }
        case "set":
            let p = rest.split(separator: ":", maxSplits: 1).map(String.init)
            guard p.count == 2, let i = Int(p[0]), m.steps.indices.contains(i) else { print("BRENAME bad set"); return }
            let kv = p[1].split(separator: "=", maxSplits: 1, omittingEmptySubsequences: false).map(String.init)
            guard kv.count == 2 else { return }
            set(&m.steps[i], kv[0], kv[1])
        case "save": m.savePreset(named: rest)
        case "load":
            if let p = m.presets.first(where: { $0.name.lowercased() == rest.lowercased() }) { m.load(p) }
            else { print("BRENAME no preset \(rest)") }
        case "presets":
            for p in m.presets { print("BRPRESET \(p.name) steps=\(p.steps.count)") }
        case "apply": BatchRenameSheet.apply(app)
        case "cancel": BatchRenameSheet.close()
        default: print("BRENAME unknown \(op)")
        }
    }

    private static func set(_ s: inout RenameStep, _ key: String, _ v: String) {
        let b = v == "true" || v == "1"
        switch key {
        case "template": s.template = v
        case "find": s.find = v
        case "replace": s.replace = v
        case "regex": s.regex = b
        case "ignoreCase": s.ignoreCase = b
        case "firstOnly": s.firstOnly = b
        case "enabled": s.enabled = b
        case "target": if let t = RenameTarget(rawValue: v) { s.target = t }
        case "caseKind": if let c = RenameCase(rawValue: v) { s.caseKind = c }
        case "start": s.start = Int64(v) ?? s.start
        case "step": s.step = Int64(v) ?? s.step
        case "padding": s.padding = UInt32(v) ?? s.padding
        case "position": if let p = RenamePosition(rawValue: v) { s.position = p }
        case "index": s.index = UInt32(v) ?? s.index
        case "separator": s.separator = v
        case "text": s.text = v
        case "trimMode": if let t = RenameTrimMode(rawValue: v) { s.trimMode = t }
        case "count": s.count = UInt32(v) ?? s.count
        case "chars": s.chars = v
        case "extChange": if let e = RenameExtChange(rawValue: v) { s.extChange = e }
        case "ext": s.ext = v
        case "kind": if let k = RenameStepKind(rawValue: v) { s.kind = k }
        default: print("BRENAME unknown field \(key)")
        }
    }
}
#endif

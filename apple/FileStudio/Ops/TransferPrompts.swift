import AppKit

/// Safe Transfer's resume questions (proposal §6.1), drawn only: the core
/// words every line and decides when to ask; each button calls the engine.
///
/// - `resumePrompt`: an interrupted copy's disks are back. Resume, Later
///   (nothing changes; the operations list keeps the row), or Discard…,
///   which asks first with Cancel as the default.
/// - `sourceMissing`: a copy waits for source disks; a toast with Show.
@MainActor
enum TransferPrompts {
    private static var installed = false
    /// Ops a prompt is open for, so a second reconnect doesn't stack another.
    private static var open: Set<OpID> = []

    static func install(_ model: AppModel) {
        guard !installed else { return }
        installed = true
        model.registerShellHandler(for: "resumePrompt") { req, _ in
            guard case .resumePrompt(let op, let q, let d, let dq, let dd) = req else { return false }
            OpsModel.of(model).refreshInterrupted()
            ask(op: op, question: q, detail: d, discardQuestion: dq, discardDetail: dd, model: model)
            return true
        }
        model.registerShellHandler(for: "sourceMissing") { req, ctx in
            guard case .sourceMissing(_, let text) = req else { return false }
            #if DEBUG
            print("SOURCEMISSING \(text)")
            #endif
            OpsModel.of(model).refreshInterrupted()
            ToastCenter.of(model).show(text, style: .warning, seconds: 8, actionTitle: "Show") {
                model.perform(Invocation(id: "view.show_operations"), via: .mouse, window: ctx.window)
            }
            return true
        }
    }

    private static func ask(op: OpID, question: String, detail: String, discardQuestion: String,
                            discardDetail: String, model: AppModel) {
        guard !open.contains(op) else { return }
        open.insert(op)
        #if DEBUG
        print("RESUMEPROMPT \(question) | \(detail)")
        #endif
        let a = NSAlert()
        a.messageText = question
        a.informativeText = detail
        a.addButton(withTitle: "Resume")
        a.addButton(withTitle: "Later")
        a.addButton(withTitle: "Discard…")
        let done: (NSApplication.ModalResponse) -> Void = { r in
            open.remove(op)
            switch r {
            case .alertFirstButtonReturn:
                OpsModel.of(model).run { try model.engine.resumeOp(op) }
                OpsModel.of(model).refreshInterrupted()
            case .alertThirdButtonReturn:
                confirmDiscard(op: op, question: discardQuestion, detail: discardDetail, model: model)
            default:
                break // Later: the row stays in the operations list.
            }
        }
        if let w = NSApp.keyWindow ?? NSApp.mainWindow {
            a.beginSheetModal(for: w, completionHandler: done)
        } else {
            done(a.runModal())
        }
    }

    private static func confirmDiscard(op: OpID, question: String, detail: String, model: AppModel) {
        let a = NSAlert()
        a.alertStyle = .warning
        a.messageText = question
        a.informativeText = detail
        // Cancel first: it is the default button (Return), and Esc cancels too.
        let cancel = a.addButton(withTitle: "Cancel")
        cancel.keyEquivalent = "\r"
        let discard = a.addButton(withTitle: "Discard")
        discard.keyEquivalent = ""
        discard.hasDestructiveAction = true
        let done: (NSApplication.ModalResponse) -> Void = { r in
            guard r == .alertSecondButtonReturn else { return }
            OpsModel.of(model).run { try model.engine.cancelOp(op) }
            OpsModel.of(model).refreshInterrupted()
        }
        if let w = NSApp.keyWindow ?? NSApp.mainWindow {
            a.beginSheetModal(for: w, completionHandler: done)
        } else {
            done(a.runModal())
        }
    }
}

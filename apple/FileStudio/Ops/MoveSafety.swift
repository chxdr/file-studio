// MoveSafety.swift — the shell half of F-023 (no silent moves of important
// folders; incident 2026-11-12). The core decides; this only shows:
// - `ShellRequest.confirmMove`: a sheet, "Move “Downloads” into “Import
//   Studio”?", with Cancel as the default button (Return and Esc both
//   cancel); Move calls `engine.confirm(token)`.
// - `ShellRequest.moved`: a toast, "Moved “X” to “Y”", for about six seconds
//   with Undo (`edit.undo`, as ⌘Z).

import AppKit

@MainActor
enum MoveSafety {
    private static var installed = false
    /// The open confirmation (debug automation answers it).
    private(set) static var openAlert: NSAlert?
    #if DEBUG
    /// Last toast text shown (debug automation logs it).
    static var lastToast: String?
    #endif

    static func install(_ model: AppModel) {
        guard !installed else { return }
        installed = true
        model.registerShellHandler(for: "confirmMove") { req, ctx in
            guard case .confirmMove(let token, let title, let detail, _) = req else { return false }
            confirm(token: token, title: title, detail: detail, model: model, window: ctx.window)
            return true
        }
        model.registerShellHandler(for: "moved") { req, ctx in
            guard case .moved(let text) = req else { return false }
            #if DEBUG
            lastToast = text
            print("MOVED \(text)")
            #endif
            ToastCenter.of(model).show(text, style: .success, seconds: 6, actionTitle: "Undo") {
                model.perform(Invocation(id: "edit.undo"), via: .mouse, window: ctx.window)
            }
            return true
        }
        // S11: Trash, Delete Immediately, Undo and Redo always say what they did.
        model.registerShellHandler(for: "toast") { req, ctx in
            guard case .toast(let text, let button, let command, let style) = req else { return false }
            #if DEBUG
            lastToast = text
            print("TOAST \(text)\(button.map { " [\($0)]" } ?? "")\(style == .warning ? " (warning)" : "")")
            #endif
            let look: Toast.Style = switch style {
            case .success: .success
            case .warning: .warning
            case .info: .info
            }
            if let button, let command {
                ToastCenter.of(model).show(text, style: look, seconds: style == .warning ? 8 : 6, actionTitle: button) {
                    model.perform(Invocation(id: command), via: .mouse, window: ctx.window)
                }
            } else {
                ToastCenter.of(model).show(text, style: look, seconds: style == .warning ? 6 : 4)
            }
            return true
        }
        // A failure toast's Details: the queue, where each failed item has its reason.
        model.registerShellHandler(for: "showOperations") { req, _ in
            guard case .showOperations = req else { return false }
            #if DEBUG
            print("SHOWOPS")
            #endif
            OpsModel.of(model).popoverShown = true
            return true
        }
    }

    private static func host(_ id: WindowID?) -> NSWindow? {
        let mains = NSApp.windows.filter { $0.windowController is CoreWindowIdentifying && $0.isVisible }
        if let id, let w = mains.first(where: { ($0.windowController as? CoreWindowIdentifying)?.coreWindowID == id }) {
            return w
        }
        return mains.first { $0.isKeyWindow } ?? mains.first
    }

    static func confirm(token: UInt64, title: String, detail: String, model: AppModel, window: WindowID?) {
        let a = NSAlert()
        a.alertStyle = .warning
        a.messageText = title
        a.informativeText = detail
        // Cancel first: it is the default button (Return), and Esc cancels too.
        let cancel = a.addButton(withTitle: "Cancel")
        cancel.keyEquivalent = "\r"
        let move = a.addButton(withTitle: "Move")
        move.keyEquivalent = ""
        move.hasDestructiveAction = true
        openAlert = a
        #if DEBUG
        print("CONFIRMMOVE \(title) | \(detail)")
        #endif
        let done: (NSApplication.ModalResponse) -> Void = { r in
            openAlert = nil
            if r == .alertSecondButtonReturn {
                do { model.handle(try model.engine.confirm(token), in: window) } catch {
                    model.statusMessage = error.localizedDescription
                }
            } else {
                model.engine.cancelConfirmation(token)
                model.statusMessage = "Nothing was moved"
            }
        }
        if let w = host(window) { a.beginSheetModal(for: w, completionHandler: done) } else { done(a.runModal()) }
    }

    /// A drop or move the core refused: say why where the user is looking (a
    /// toast) and in the status line, never just a beep (safety audit S2).
    static func refused(_ text: String, model: AppModel) {
        model.statusMessage = text
        #if DEBUG
        print("REFUSED \(text)")
        #endif
        ToastCenter.of(model).show(text, style: .warning, seconds: 5)
    }

    /// Answers the open confirmation as if a button were pressed (debug automation).
    static func answer(move: Bool) {
        guard let a = openAlert, let parent = a.window.sheetParent else { return }
        parent.endSheet(a.window, returnCode: move ? .alertSecondButtonReturn : .alertFirstButtonReturn)
    }
}

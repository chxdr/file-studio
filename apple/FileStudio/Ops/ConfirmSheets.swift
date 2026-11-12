// ConfirmSheets.swift — the two questions the core can ask before it acts:
// "Move to the Trash?" (when Settings › Ask before moving to the Trash is on,
// and always for Vim `x`) and "Delete Immediately?" (always). They are
// `ShellRequest`s carrying a one-shot token; on yes the shell calls
// `engine.confirm(token)` and the core acts on exactly the items the question
// named, never on whatever is selected by then (safety audit S1). Nothing is
// decided here beyond showing the question.

import AppKit

@MainActor
enum ConfirmSheets {
    static func install(_ model: AppModel) {
        model.registerShellHandler(for: "confirmTrash") { req, ctx in
            guard case .confirmTrash(let token, let paths) = req else { return false }
            ask(token: token, paths: paths, ctx: ctx, model: model,
                title: paths.count == 1 ? "Move “\(name(paths[0]))” to the Trash?" : "Move \(paths.count) items to the Trash?",
                detail: "You can put them back from the Trash, or undo with ⌘Z.",
                yes: "Move to Trash", destructive: false)
            return true
        }
        model.registerShellHandler(for: "confirmDeleteImmediately") { req, ctx in
            guard case .confirmDeleteImmediately(let token, let paths) = req else { return false }
            ask(token: token, paths: paths, ctx: ctx, model: model,
                title: paths.count == 1 ? "Delete “\(name(paths[0]))” immediately?" : "Delete \(paths.count) items immediately?",
                detail: "This can’t be undone. The items will not go to the Trash.",
                yes: "Delete Immediately", destructive: true)
            return true
        }
    }

    private static func name(_ p: String) -> String { (p as NSString).lastPathComponent }

    /// The open question (debug automation answers it).
    private(set) static var openAlert: NSAlert?

    private static func ask(token: UInt64, paths: [String], ctx: ShellContext, model: AppModel, title: String,
                            detail: String, yes: String, destructive: Bool) {
        let alert = NSAlert()
        alert.messageText = title
        alert.informativeText = detail
        alert.alertStyle = destructive ? .critical : .warning
        // Deleting for good: Cancel is the default (Return); Trash: the action is.
        let first = alert.addButton(withTitle: destructive ? "Cancel" : yes)
        let second = alert.addButton(withTitle: destructive ? yes : "Cancel")
        if destructive { second.hasDestructiveAction = true }
        openAlert = alert
        #if DEBUG
        print("CONFIRM \(title) | \(paths.map { ($0 as NSString).lastPathComponent }.joined(separator: ", "))")
        #endif
        let done: (NSApplication.ModalResponse) -> Void = { r in
            openAlert = nil
            let confirmed = destructive ? r == .alertSecondButtonReturn : r == .alertFirstButtonReturn
            guard confirmed else {
                model.engine.cancelConfirmation(token)
                return
            }
            do { model.handle(try model.engine.confirm(token), in: ctx.window) } catch {
                model.statusMessage = error.localizedDescription
            }
        }
        _ = first
        if let w = NSApp.keyWindow ?? NSApp.windows.first(where: { $0.windowController is CoreWindowIdentifying && $0.isVisible }) {
            alert.beginSheetModal(for: w, completionHandler: done)
        } else {
            done(alert.runModal())
        }
    }

    /// Answers the open question as if a button were pressed (debug automation).
    static func answer(yes: Bool) {
        guard let a = openAlert, let parent = a.window.sheetParent else { return }
        let destructive = a.alertStyle == .critical
        let code: NSApplication.ModalResponse = (yes != destructive) ? .alertFirstButtonReturn : .alertSecondButtonReturn
        parent.endSheet(a.window, returnCode: code)
    }
}

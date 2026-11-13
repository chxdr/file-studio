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
        model.registerShellHandler(for: "confirm") { req, ctx in
            guard case .confirm(let token, let title, let detail, let yes, let no) = req else { return false }
            askGeneric(token: token, title: title, detail: detail, yes: yes, no: no, ctx: ctx, model: model)
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

    private static func name(_ p: String) -> String { coreDisplayName(p) }

    /// The core's generic question (S14: renames that hide an item or change
    /// its extension, recreating a folder for an undo). `no` is the default.
    private static func askGeneric(token: UInt64, title: String, detail: String, yes: String, no: String,
                                   ctx: ShellContext, model: AppModel) {
        let alert = NSAlert()
        alert.messageText = title
        alert.informativeText = detail
        alert.alertStyle = .warning
        alert.addButton(withTitle: no)   // first: the default (Return); Esc cancels too
        alert.addButton(withTitle: yes)
        openAlert = alert
        yesIsFirst = false
        #if DEBUG
        print("CONFIRM \(title) | \(no) / \(yes)")
        #endif
        let done: (NSApplication.ModalResponse) -> Void = { r in
            openAlert = nil
            guard r == .alertSecondButtonReturn else {
                model.engine.cancelConfirmation(token)
                return
            }
            do { model.handle(try model.engine.confirm(token), in: ctx.window) } catch {
                model.statusMessage = error.localizedDescription
            }
        }
        if let w = NSApp.keyWindow ?? NSApp.windows.first(where: { $0.windowController is CoreWindowIdentifying && $0.isVisible }) {
            alert.beginSheetModal(for: w, completionHandler: done)
        } else {
            done(alert.runModal())
        }
    }

    /// The open question (debug automation answers it).
    private(set) static var openAlert: NSAlert?
    /// Whether the open question's first button is its yes.
    private static var yesIsFirst = true

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
        yesIsFirst = !destructive
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
        let code: NSApplication.ModalResponse = (yes == yesIsFirst) ? .alertFirstButtonReturn : .alertSecondButtonReturn
        parent.endSheet(a.window, returnCode: code)
    }
}

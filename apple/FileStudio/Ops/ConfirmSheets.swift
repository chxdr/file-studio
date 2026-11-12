// ConfirmSheets.swift — the two questions the core can ask before it acts:
// "Move to the Trash?" (when Settings › Ask before moving to the Trash is on)
// and "Delete Immediately?" (always). They are `ShellRequest`s; on yes the
// shell performs the same command again with `.confirmed`, and the core
// carries it out. Nothing is decided here beyond showing the question.

import AppKit

@MainActor
enum ConfirmSheets {
    static func install(_ model: AppModel) {
        model.registerShellHandler(for: "confirmTrash") { req, ctx in
            guard case .confirmTrash(let paths) = req else { return false }
            ask(paths: paths, ctx: ctx, model: model, command: "file.trash",
                title: paths.count == 1 ? "Move “\(name(paths[0]))” to the Trash?" : "Move \(paths.count) items to the Trash?",
                detail: "You can put them back from the Trash, or undo with ⌘Z.",
                yes: "Move to Trash", destructive: false)
            return true
        }
        model.registerShellHandler(for: "confirmDeleteImmediately") { req, ctx in
            guard case .confirmDeleteImmediately(let paths) = req else { return false }
            ask(paths: paths, ctx: ctx, model: model, command: "file.delete_immediately",
                title: paths.count == 1 ? "Delete “\(name(paths[0]))” immediately?" : "Delete \(paths.count) items immediately?",
                detail: "This can’t be undone. The items will not go to the Trash.",
                yes: "Delete Immediately", destructive: true)
            return true
        }
    }

    private static func name(_ p: String) -> String { (p as NSString).lastPathComponent }

    private static func ask(paths: [String], ctx: ShellContext, model: AppModel, command: String, title: String,
                            detail: String, yes: String, destructive: Bool) {
        let alert = NSAlert()
        alert.messageText = title
        alert.informativeText = detail
        alert.alertStyle = destructive ? .critical : .warning
        // Deleting for good: Cancel is the default (Return); Trash: the action is.
        let first = alert.addButton(withTitle: destructive ? "Cancel" : yes)
        let second = alert.addButton(withTitle: destructive ? yes : "Cancel")
        if destructive { second.hasDestructiveAction = true }
        let done: (NSApplication.ModalResponse) -> Void = { r in
            let confirmed = destructive ? r == .alertSecondButtonReturn : r == .alertFirstButtonReturn
            guard confirmed else { return }
            model.perform(Invocation(id: command, arg: .confirmed, tab: ctx.tab), via: .menu, window: ctx.window)
        }
        _ = first
        if let w = NSApp.keyWindow ?? NSApp.windows.first(where: { $0.windowController is CoreWindowIdentifying && $0.isVisible }) {
            alert.beginSheetModal(for: w, completionHandler: done)
        } else {
            done(alert.runModal())
        }
    }
}

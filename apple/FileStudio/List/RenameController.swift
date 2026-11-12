// RenameController.swift — rename in place (F-018: Return renames; the core
// decides, sending `ShellRequest.beginRename`). The field selects the name
// without its extension, as Finder does. Return or focus loss commits
// through `engine.renameItem` (the core validates and queues the rename);
// Esc cancels. A refused name is shown under the field and editing goes on.
// Keys in the field go to the core first with `.rename` focus, which answers
// `textField(rename.commit / rename.cancel)`.

import AppKit

/// The text field: reports `.rename` focus for its tab.
@MainActor
final class RenameField: NSTextField, CoreFocusReporting {
    var coreFocus: Focus { .rename }
    var coreTab: TabID?
}

@MainActor
final class RenameController: NSObject, NSTextFieldDelegate {
    private unowned let model: AppModel
    private unowned let table: FileTable
    let field = RenameField()
    private let errorLabel = NSTextField(labelWithString: "")
    private(set) var oldName: String?
    private var finishing = false

    init(model: AppModel, table: FileTable) {
        self.model = model
        self.table = table
        super.init()
        field.font = CellStyle.nameFont
        field.isBordered = true
        field.bezelStyle = .squareBezel
        field.focusRingType = .exterior
        field.drawsBackground = true
        field.backgroundColor = .textBackgroundColor
        field.lineBreakMode = .byClipping
        field.cell?.isScrollable = true
        field.cell?.wraps = false
        field.delegate = self
        field.setAccessibilityLabel("Name")
        errorLabel.font = .systemFont(ofSize: 11)
        errorLabel.textColor = .white
        errorLabel.drawsBackground = true
        errorLabel.backgroundColor = .systemRed
        errorLabel.wantsLayer = true
        errorLabel.layer?.cornerRadius = 3
    }

    var isActive: Bool { oldName != nil }

    /// Starts editing `name` (scrolls it into view).
    func begin(name: String) {
        guard let src = table.source, let i = src.indexOf(name: name) else {
            NSSound.beep()
            return
        }
        cancel()
        oldName = name
        field.coreTab = table.ownerTab
        field.stringValue = name
        errorLabel.isHidden = true
        table.table.scrollRowToVisible(Int(i))
        table.table.addSubview(field)
        table.table.addSubview(errorLabel)
        reposition()
        table.table.window?.makeFirstResponder(field)
        if let editor = field.currentEditor() {
            editor.selectedRange = Self.selectionRange(for: name, isFolder: src.listingRowIsFolder(Int(i), table))
        }
    }

    /// Finder's rule: the name without its extension; all of it for folders
    /// and names without one (or starting with a dot).
    static func selectionRange(for name: String, isFolder: Bool) -> NSRange {
        let ns = name as NSString
        guard !isFolder else { return NSRange(location: 0, length: ns.length) }
        let dot = ns.range(of: ".", options: .backwards)
        if dot.location == NSNotFound || dot.location == 0 { return NSRange(location: 0, length: ns.length) }
        return NSRange(location: 0, length: dot.location)
    }

    /// Keeps the field over the row as rows move.
    func reposition() {
        guard let name = oldName, let src = table.source else { return }
        guard let i = src.indexOf(name: name) else { cancel(); return }
        let t = table.table
        let col = max(t.column(withIdentifier: NSUserInterfaceItemIdentifier(ColumnID.name.rawValue)), 0)
        var r = t.frameOfCell(atColumn: col, row: Int(i))
        let h: CGFloat = 20
        r = NSRect(x: r.minX + 19, y: r.midY - h / 2, width: max(min(r.width - 21, 600), 80), height: h)
        field.frame = r
        if !errorLabel.isHidden {
            errorLabel.sizeToFit()
            errorLabel.frame = NSRect(x: r.minX, y: r.maxY + 2, width: errorLabel.frame.width + 8, height: 18)
        }
    }

    /// Commits if editing (a click elsewhere, a menu opening).
    func commitIfActive() { if isActive { commit() } }

    func commit() {
        guard let old = oldName, !finishing, let tab = table.ownerTab else { return }
        let new = field.stringValue
        finishing = true
        defer { finishing = false }
        do {
            let out = try model.engine.renameItem(tab: tab, oldName: old, newName: new)
            model.handle(out, in: model.keyWindowID)
            end()
        } catch let e as CoreError {
            showError(e.message)
        } catch {
            showError(error.localizedDescription)
        }
    }

    func cancel() {
        guard isActive else { return }
        end()
    }

    private func end() {
        let hadFocus = field.currentEditor() != nil
        oldName = nil
        field.removeFromSuperview()
        errorLabel.removeFromSuperview()
        if hadFocus { table.table.window?.makeFirstResponder(table.table) }
    }

    private func showError(_ msg: String) {
        NSSound.beep()
        errorLabel.stringValue = " \(msg) "
        errorLabel.isHidden = false
        reposition()
        print("RENAME error: \(msg)")
        table.table.window?.makeFirstResponder(field)
    }

    // Focus loss commits (Finder). Return/Esc normally arrive as core requests.
    func controlTextDidEndEditing(_ obj: Notification) {
        guard isActive, !finishing else { return }
        let move = (obj.userInfo?["NSTextMovement"] as? Int) ?? NSTextMovement.other.rawValue
        if move == NSTextMovement.cancel.rawValue { cancel() } else { commit() }
    }

    func control(_ control: NSControl, textView: NSTextView, doCommandBy sel: Selector) -> Bool {
        if sel == #selector(NSResponder.cancelOperation(_:)) { cancel(); return true }
        if sel == #selector(NSResponder.insertNewline(_:)) { commit(); return true }
        return false
    }

    func controlTextDidChange(_ obj: Notification) {
        if !errorLabel.isHidden { errorLabel.isHidden = true }
    }
}

extension RowSource {
    /// Whether a row is a folder (selects the whole name when renaming).
    @MainActor
    func listingRowIsFolder(_ i: Int, _ t: FileTable) -> Bool { t.row(i)?.kind == .folder }
}

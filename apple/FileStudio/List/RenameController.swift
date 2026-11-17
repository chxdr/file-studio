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

/// What the rename field sits on: a file table, or the icon view and the
/// gallery (Grid/). Gives the rows, where the name is drawn and who gets
/// focus back.
@MainActor
protocol RenameHost: AnyObject {
    /// The rows (nil once detached).
    var renameSource: RowSource? { get }
    /// The tab the rename acts through.
    var renameOwnerTab: TabID? { get }
    /// The view the field is added to (it scrolls with the items).
    var renameContainer: NSView { get }
    /// Takes focus back when editing ends.
    var renameFocusView: NSView { get }
    /// Scrolls row `i` into view.
    func renameReveal(_ i: Int)
    /// Where the field goes for row `i`, in the container's coordinates.
    func renameFieldFrame(_ i: Int) -> NSRect?
    /// Row `i` is a folder (the whole name is selected).
    func renameIsFolder(_ i: Int) -> Bool
}

@MainActor
final class RenameController: NSObject, NSTextFieldDelegate {
    private unowned let model: AppModel
    private unowned let host: RenameHost
    let field = RenameField()
    private let errorLabel = NSTextField(labelWithString: "")
    private(set) var oldName: String?
    private var finishing = false

    convenience init(model: AppModel, table: FileTable) {
        self.init(model: model, host: table)
    }

    init(model: AppModel, host: RenameHost) {
        self.model = model
        self.host = host
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
        guard let src = host.renameSource, let i = src.indexOf(name: name) else {
            NSSound.beep()
            return
        }
        cancel()
        oldName = name
        // M3 search: a result's row name is its path below the search root; edit its own name.
        let name = (name as NSString).lastPathComponent
        field.coreTab = host.renameOwnerTab
        field.stringValue = name
        errorLabel.isHidden = true
        host.renameReveal(Int(i))
        host.renameContainer.addSubview(field)
        host.renameContainer.addSubview(errorLabel)
        reposition()
        host.renameContainer.window?.makeFirstResponder(field)
        if let editor = field.currentEditor() {
            editor.selectedRange = Self.selectionRange(for: name, isFolder: host.renameIsFolder(Int(i)))
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
        guard let name = oldName, let src = host.renameSource else { return }
        guard let i = src.indexOf(name: name), let r = host.renameFieldFrame(Int(i)) else { cancel(); return }
        field.frame = r
        if !errorLabel.isHidden {
            errorLabel.sizeToFit()
            errorLabel.frame = NSRect(x: r.minX, y: r.maxY + 2, width: errorLabel.frame.width + 8, height: 18)
        }
    }

    /// Commits if editing (a click elsewhere, a menu opening).
    func commitIfActive() { if isActive { commit() } }

    func commit() {
        guard let old = oldName, !finishing, let tab = host.renameOwnerTab else { return }
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
        if hadFocus { host.renameFocusView.window?.makeFirstResponder(host.renameFocusView) }
    }

    private func showError(_ msg: String) {
        NSSound.beep()
        errorLabel.stringValue = " \(msg) "
        errorLabel.isHidden = false
        reposition()
        print("RENAME error: \(msg)")
        host.renameContainer.window?.makeFirstResponder(field)
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

extension FileTable: RenameHost {
    var renameSource: RowSource? { source }
    var renameOwnerTab: TabID? { ownerTab }
    var renameContainer: NSView { table }
    var renameFocusView: NSView { table }
    func renameReveal(_ i: Int) { table.scrollRowToVisible(i) }
    func renameFieldFrame(_ i: Int) -> NSRect? {
        let col = max(table.column(withIdentifier: NSUserInterfaceItemIdentifier(ColumnID.name.rawValue)), 0)
        let r = table.frameOfCell(atColumn: col, row: i)
        let h: CGFloat = 20
        return NSRect(x: r.minX + 19, y: r.midY - h / 2, width: max(min(r.width - 21, 600), 80), height: h)
    }
    /// Whether a row is a folder (selects the whole name when renaming).
    func renameIsFolder(_ i: Int) -> Bool { row(i)?.kind == .folder }
}

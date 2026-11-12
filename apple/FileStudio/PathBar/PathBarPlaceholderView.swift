// PathBarPlaceholderView.swift — PLACEHOLDER for the address bar (F-018).
// The address-bar session replaces this with the real custom view (crumbs,
// chevron menus, completion panel, paste chip). What it shows the wiring for:
// - `CoreTextField` reports `.addressBar` focus, so KeyRouter sends Return,
//   Esc and Tab to the core, which answers with `ShellRequest.textField`.
// - ⌘L / `/` arrive as `ShellRequest.focusAddressBar(pane:text:)`.
// - Commit goes back through the core (`addressbar.commit` with the text).

import AppKit

/// A text field that tells KeyRouter which focus it is.
@MainActor
final class CoreTextField: NSTextField, CoreFocusReporting {
    var coreFocus: Focus = .addressBar
    var coreTab: TabID?
    var onBecomeFirstResponder: (() -> Void)?

    override func becomeFirstResponder() -> Bool {
        let ok = super.becomeFirstResponder()
        if ok { onBecomeFirstResponder?() }
        return ok
    }
}

@MainActor
final class PathBarPlaceholderView: NSView, NSTextFieldDelegate {
    private unowned let model: AppModel
    let paneID: PaneID
    let field = CoreTextField()
    private var tab: TabModel?
    private var handlers: [Int] = []
    private var observer: ObserverToken?
    private var editing = false

    init(model: AppModel, paneID: PaneID) {
        self.model = model
        self.paneID = paneID
        super.init(frame: .zero)
        field.isBordered = false
        field.drawsBackground = false
        field.font = .systemFont(ofSize: 13)
        field.focusRingType = .none
        field.lineBreakMode = .byTruncatingMiddle
        field.cell?.isScrollable = true
        field.delegate = self
        field.placeholderString = "Path"
        field.onBecomeFirstResponder = { [weak self] in self?.beginEditing() }
        field.translatesAutoresizingMaskIntoConstraints = false
        addSubview(field)
        NSLayoutConstraint.activate([
            field.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 12),
            field.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -12),
            field.centerYAnchor.constraint(equalTo: centerYAnchor),
        ])
        handlers.append(model.registerShellHandler(for: "focusAddressBar") { [weak self] req, _ in
            guard let self, case .focusAddressBar(let pane, let text) = req, pane == self.paneID else { return false }
            self.window?.makeFirstResponder(self.field)
            if !text.isEmpty {
                self.field.stringValue = text
                self.field.currentEditor()?.selectedRange = NSRange(location: text.utf16.count, length: 0)
            }
            return true
        })
        handlers.append(model.registerShellHandler(for: "textField") { [weak self] req, _ in
            guard let self, self.editing, case .textField(let cmd) = req else { return false }
            self.textCommand(cmd)
            return true
        })
        observer = model.observe { [weak self] e in
            guard let self, let t = self.tab else { return }
            if case .listingChanged(let id, _, _) = e, id == t.id, !self.editing { self.showRest() }
        }
    }
    required init?(coder: NSCoder) { fatalError() }

    isolated deinit { handlers.forEach { model.unregisterShellHandler($0) } }

    func show(_ t: TabModel) {
        tab = t
        field.coreTab = t.id
        showRest()
    }

    /// At rest: the crumbs as text (the real bar draws buttons).
    private func showRest() {
        guard let t = tab else { return }
        field.stringValue = t.api.breadcrumbs().map(\.name).joined(separator: "  ›  ")
        field.textColor = .labelColor
    }

    private func beginEditing() {
        guard let t = tab else { return }
        editing = true
        field.stringValue = t.location
        field.currentEditor()?.selectAll(nil)
    }

    func controlTextDidEndEditing(_ obj: Notification) {
        editing = false
        showRest()
    }

    private func textCommand(_ cmd: String) {
        guard let t = tab else { return }
        switch cmd {
        case "addressbar.commit", "addressbar.commit_other_pane":
            let text = field.stringValue
            editing = false
            window?.makeFirstResponder(nil)
            model.perform(Invocation(id: cmd, arg: .path(text), tab: t.id), via: .keyboard)
            focusList()
        case "addressbar.cancel":
            editing = false
            window?.makeFirstResponder(nil)
            showRest()
            focusList()
        case "addressbar.complete_next", "addressbar.accept_ghost":
            let text = field.stringValue
            let c = t.api.complete(text, cursor: UInt32(text.utf8.count))
            if let ext = c.commonPrefixExtension {
                field.stringValue = ext.newInput
            } else if let first = c.items.first {
                field.stringValue = first.insertText
            }
            field.currentEditor()?.selectedRange = NSRange(location: field.stringValue.utf16.count, length: 0)
        default:
            break
        }
    }

    private func focusList() {
        if let pane = window?.windowController as? MainWindowController,
           let p = pane.allPanes.first(where: { $0.paneID == paneID }) {
            window?.makeFirstResponder(p.list.table)
        }
    }
}

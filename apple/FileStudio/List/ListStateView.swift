// ListStateView.swift — what a file table shows instead of rows: an empty
// folder, a slow load (spinner after ~150 ms, set by FileTable), or an error
// with a "Grant Full Disk Access" hint when macOS refused access.

import AppKit

@MainActor
final class ListStateView: NSView {
    enum State: Equatable {
        case none, loading, empty
        /// Every item is hidden or filtered out.
        case emptyFiltered(Int)
        case error(String)
    }

    private(set) var state: State = .none
    private let spinner = NSProgressIndicator()
    private let title = NSTextField(labelWithString: "")
    private let detail = NSTextField(wrappingLabelWithString: "")
    private let button = NSButton(title: "Open Privacy & Security Settings…", target: nil, action: nil)
    private let stack = NSStackView()

    override init(frame: NSRect) {
        super.init(frame: frame)
        spinner.style = .spinning
        spinner.controlSize = .regular
        spinner.isDisplayedWhenStopped = false
        title.font = .systemFont(ofSize: 13, weight: .medium)
        title.textColor = .secondaryLabelColor
        title.alignment = .center
        detail.font = .systemFont(ofSize: 11)
        detail.textColor = .tertiaryLabelColor
        detail.alignment = .center
        detail.preferredMaxLayoutWidth = 320
        button.bezelStyle = .push
        button.controlSize = .small
        button.target = self
        button.action = #selector(openPrivacy)
        stack.orientation = .vertical
        stack.alignment = .centerX
        stack.spacing = 6
        for v in [spinner, title, detail, button] { stack.addArrangedSubview(v) }
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)
        NSLayoutConstraint.activate([
            stack.leadingAnchor.constraint(equalTo: leadingAnchor), stack.trailingAnchor.constraint(equalTo: trailingAnchor),
            stack.topAnchor.constraint(equalTo: topAnchor), stack.bottomAnchor.constraint(equalTo: bottomAnchor),
        ])
        show(.none)
    }
    required init?(coder: NSCoder) { fatalError() }

    /// macOS privacy protection refused access (EPERM: "Operation not
    /// permitted"); Full Disk Access is the fix.
    static func isPrivacyError(_ s: String) -> Bool {
        let l = s.lowercased()
        return l.contains("not permitted") || l.contains("os error 1)")
    }
    /// The folder's own permissions refused access (EACCES).
    static func isPermissionError(_ s: String) -> Bool {
        let l = s.lowercased()
        return l.contains("permission denied") || l.contains("os error 13)")
    }
    /// The reason without the path the core puts in front of it.
    static func reason(_ s: String) -> String {
        guard let r = s.range(of: ": ", options: .backwards) else { return s }
        return String(s[r.upperBound...])
    }

    func show(_ s: State) {
        state = s
        isHidden = s == .none
        spinner.isHidden = s != .loading
        if s == .loading { spinner.startAnimation(nil) } else { spinner.stopAnimation(nil) }
        button.isHidden = true
        detail.isHidden = true
        switch s {
        case .none, .loading:
            title.stringValue = s == .loading ? "Loading…" : ""
            title.isHidden = s == .none
        case .empty:
            title.stringValue = "Folder is empty"
            title.isHidden = false
        case .emptyFiltered(let n):
            title.stringValue = "No items shown"
            detail.stringValue = n == 1 ? "1 item is hidden or filtered out." : "\(n) items are hidden or filtered out."
            title.isHidden = false; detail.isHidden = false
        case .error(let msg):
            let privacy = Self.isPrivacyError(msg)
            if privacy {
                title.stringValue = "File Studio can’t open this folder"
                detail.stringValue = "macOS didn’t allow access. Grant File Studio Full Disk Access in "
                    + "System Settings › Privacy & Security, then open the folder again."
            } else if Self.isPermissionError(msg) {
                title.stringValue = "You don’t have permission to see this folder"
                detail.stringValue = "Its permissions don’t let you read it. Use Get Info to see who can."
            } else {
                title.stringValue = "This folder can’t be read"
                detail.stringValue = Self.reason(msg)
            }
            title.isHidden = false; detail.isHidden = false
            button.isHidden = !privacy
        }
        setAccessibilityLabel(title.stringValue)
    }

    @objc private func openPrivacy() {
        if let u = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles") {
            NSWorkspace.shared.open(u)
        }
    }

    // Clicks fall through to the table (empty-space menus and drops still work).
    override func hitTest(_ point: NSPoint) -> NSView? {
        let v = super.hitTest(point)
        return v === button || v?.isDescendant(of: button) == true ? v : nil
    }
}

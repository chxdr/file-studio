// ConnectServerSheet.swift — Connect to Server… (⌘K, `nav.connect_server`).
//
// A small sheet: an address field (checked by the core as you type), the
// recent servers (click to fill, double-click to connect), "Add to Sidebar",
// and Connect. Connect hands the address to `connectServer`, which mounts
// the share with macOS's NetFS — the system shows its own sign-in window
// and share picker — and sends the tab there when it is mounted. The sheet
// closes as soon as the connection starts; a failure comes back as a toast.

import AppKit

@MainActor
final class ConnectServerSheet: NSObject, NSTableViewDataSource, NSTableViewDelegate, NSTextFieldDelegate {
    private static var installed = false
    /// The open sheet, if any (debug automation drives it).
    private(set) static var current: ConnectServerSheet?

    /// Handles `ShellRequest.connectToServer` (call once at launch).
    static func install(_ model: AppModel) {
        guard !installed else { return }
        installed = true
        model.registerShellHandler(for: "connectToServer") { _, ctx in
            show(model: model, window: ctx.window, tab: ctx.tab)
            return true
        }
    }

    /// Opens the sheet on the window (or brings the open one forward).
    static func show(model: AppModel, window: WindowID?, tab: TabID?) {
        if let c = current { c.panel.makeKeyAndOrderFront(nil); return }
        guard let net = model.network else { return }
        let s = ConnectServerSheet(model: model, net: net, window: window, tab: tab ?? model.focusedTab?.id)
        current = s
        s.present(on: SidebarServices.hostWindow(window))
    }

    private unowned let model: AppModel
    private let net: NetworkEngineAPI
    private let windowID: WindowID?
    private let tab: TabID?
    private var recents: [NetworkServer] = []
    private weak var host: NSWindow?

    let panel = NSPanel(contentRect: NSRect(x: 0, y: 0, width: 460, height: 360),
                        styleMask: [.titled], backing: .buffered, defer: true)
    let field = NSTextField(string: "")
    private let status = NSTextField(labelWithString: "")
    private let table = NSTableView()
    private let recentsLabel = NSTextField(labelWithString: "Recent Servers")
    private let clearButton = NSButton(title: "Clear Recents", target: nil, action: nil)
    let addToSidebar = NSButton(checkboxWithTitle: "Add to Sidebar", target: nil, action: nil)
    private let connectButton = NSButton(title: "Connect", target: nil, action: nil)
    private let cancelButton = NSButton(title: "Cancel", target: nil, action: nil)

    private init(model: AppModel, net: NetworkEngineAPI, window: WindowID?, tab: TabID?) {
        self.model = model; self.net = net; self.windowID = window; self.tab = tab
        super.init()
        build()
    }

    // MARK: Building

    private func build() {
        let title = NSTextField(labelWithString: "Connect to Server")
        title.font = .systemFont(ofSize: 13, weight: .semibold)
        field.placeholderString = "smb://server/share"
        field.font = .systemFont(ofSize: 13)
        field.delegate = self
        field.setAccessibilityLabel("Server address")
        field.lineBreakMode = .byTruncatingMiddle
        field.cell?.usesSingleLineMode = true
        status.font = .systemFont(ofSize: 11)
        status.textColor = .secondaryLabelColor
        status.lineBreakMode = .byTruncatingMiddle
        recentsLabel.font = .systemFont(ofSize: 11, weight: .semibold)
        recentsLabel.textColor = .secondaryLabelColor

        let col = NSTableColumn(identifier: .init("server"))
        table.addTableColumn(col)
        table.headerView = nil
        table.style = .inset
        table.rowHeight = 36
        table.usesAlternatingRowBackgroundColors = false
        table.dataSource = self
        table.delegate = self
        table.target = self
        table.doubleAction = #selector(doubleClicked)
        table.setAccessibilityLabel("Recent servers")
        let scroll = NSScrollView()
        scroll.documentView = table
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        scroll.borderType = .noBorder
        scroll.drawsBackground = false

        clearButton.target = self
        clearButton.action = #selector(clearRecents)
        clearButton.bezelStyle = .push
        clearButton.controlSize = .small
        addToSidebar.toolTip = "Keep this server in the sidebar’s Network section. Only the address is saved, never the password."
        cancelButton.target = self
        cancelButton.action = #selector(cancel)
        cancelButton.keyEquivalent = "\u{1b}"
        connectButton.target = self
        connectButton.action = #selector(connect)
        connectButton.keyEquivalent = "\r"

        let buttons = NSStackView(views: [clearButton, addToSidebar, NSView(), cancelButton, connectButton])
        buttons.orientation = .horizontal
        buttons.spacing = 8
        buttons.setHuggingPriority(.defaultLow, for: .horizontal)

        let stack = NSStackView(views: [title, field, status, recentsLabel, scroll, buttons])
        stack.orientation = .vertical
        stack.alignment = .leading
        stack.spacing = 8
        stack.setCustomSpacing(12, after: title)
        stack.setCustomSpacing(14, after: status)
        stack.setCustomSpacing(14, after: scroll)
        stack.edgeInsets = NSEdgeInsets(top: 18, left: 20, bottom: 18, right: 20)
        stack.translatesAutoresizingMaskIntoConstraints = false
        let content = NSView()
        content.addSubview(stack)
        for v in [field, status, scroll, buttons] as [NSView] {
            v.widthAnchor.constraint(equalTo: stack.widthAnchor, constant: -40).isActive = true
        }
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: content.topAnchor),
            stack.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            stack.bottomAnchor.constraint(equalTo: content.bottomAnchor),
            scroll.heightAnchor.constraint(equalToConstant: 160),
        ])
        panel.contentView = content
        panel.title = "Connect to Server"
        panel.initialFirstResponder = field
    }

    private func present(on host: NSWindow?) {
        self.host = host
        reloadRecents()
        // Start from the last server, selected, so typing replaces it.
        field.stringValue = recents.first?.url ?? "smb://"
        check()
        if let host {
            host.beginSheet(panel) { _ in }
        } else {
            panel.center()
            panel.makeKeyAndOrderFront(nil)
        }
        panel.makeFirstResponder(field)
        field.currentEditor()?.selectAll(nil)
    }

    private func reloadRecents() {
        recents = net.recentServers()
        table.reloadData()
        recentsLabel.isHidden = recents.isEmpty
        clearButton.isEnabled = !recents.isEmpty
    }

    // MARK: Checking as you type

    func controlTextDidChange(_ obj: Notification) { check() }

    /// Asks the core whether the address can be used, and says so under the field.
    private func check() {
        let text = field.stringValue
        do {
            let url = try net.checkServerAddress(text)
            status.textColor = .secondaryLabelColor
            // The core's form never has a password in it; macOS asks for one.
            status.stringValue = "Connects to \(url)"
            connectButton.isEnabled = true
        } catch {
            status.textColor = text.trimmingCharacters(in: .whitespaces).isEmpty || text == "smb://"
                ? .secondaryLabelColor : .systemRed
            status.stringValue = error.localizedDescription
            connectButton.isEnabled = false
        }
    }

    // MARK: Actions

    @objc func connect() {
        let address = field.stringValue
        do {
            let out = try net.connectServer(address, tab: tab)
            if addToSidebar.state == .on { try? net.saveServer(address) }
            close()
            model.handle(out, in: windowID)
        } catch {
            status.textColor = .systemRed
            status.stringValue = error.localizedDescription
            NSSound.beep()
        }
    }

    @objc func cancel() { close() }

    @objc private func clearRecents() {
        try? net.clearRecentServers()
        reloadRecents()
    }

    @objc private func doubleClicked() {
        guard table.clickedRow >= 0, table.clickedRow < recents.count else { return }
        field.stringValue = recents[table.clickedRow].url
        connect()
    }

    func tableViewSelectionDidChange(_ notification: Notification) {
        guard table.selectedRow >= 0, table.selectedRow < recents.count else { return }
        field.stringValue = recents[table.selectedRow].url
        check()
    }

    private func close() {
        if let parent = panel.sheetParent { parent.endSheet(panel) } else { panel.orderOut(nil) }
        Self.current = nil
    }

    // MARK: Recent servers table

    func numberOfRows(in tableView: NSTableView) -> Int { recents.count }

    func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
        let s = recents[row]
        let cell = tableView.makeView(withIdentifier: .init("recent"), owner: nil) as? RecentServerCell ?? RecentServerCell()
        cell.show(s)
        return cell
    }

    /// The recent rows as text (debug automation).
    var debugRows: [String] { recents.map { "\($0.name) | \($0.url)" + ($0.mountedAt.map { " | mounted at \($0)" } ?? "") } }
    var debugStatus: String { status.stringValue }
}

/// A recent server: its name, its address underneath, a symbol.
@MainActor
private final class RecentServerCell: NSTableCellView {
    private let icon = NSImageView()
    private let name = NSTextField(labelWithString: "")
    private let url = NSTextField(labelWithString: "")

    init() {
        super.init(frame: .zero)
        identifier = .init("recent")
        name.font = .systemFont(ofSize: 13)
        name.lineBreakMode = .byTruncatingTail
        url.font = .systemFont(ofSize: 11)
        url.textColor = .secondaryLabelColor
        url.lineBreakMode = .byTruncatingMiddle
        icon.symbolConfiguration = .init(pointSize: 15, weight: .regular)
        icon.contentTintColor = .controlAccentColor
        let text = NSStackView(views: [name, url])
        text.orientation = .vertical
        text.alignment = .leading
        text.spacing = 0
        let row = NSStackView(views: [icon, text])
        row.orientation = .horizontal
        row.spacing = 8
        row.translatesAutoresizingMaskIntoConstraints = false
        addSubview(row)
        textField = name
        imageView = icon
        NSLayoutConstraint.activate([
            icon.widthAnchor.constraint(equalToConstant: 20),
            row.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 4),
            row.trailingAnchor.constraint(lessThanOrEqualTo: trailingAnchor, constant: -4),
            row.centerYAnchor.constraint(equalTo: centerYAnchor),
        ])
    }
    required init?(coder: NSCoder) { fatalError() }

    func show(_ s: NetworkServer) {
        name.stringValue = s.name
        url.stringValue = s.url
        icon.image = NSImage(systemSymbolName: s.mountedAt != nil ? "externaldrive.connected.to.line.below" : "server.rack",
                             accessibilityDescription: nil)
        setAccessibilityLabel(s.mountedAt != nil ? "\(s.name), connected" : s.name)
    }
}

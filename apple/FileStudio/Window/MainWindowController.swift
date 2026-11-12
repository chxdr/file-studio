// MainWindowController.swift — one AppKit window per core window (F-002).
//
// PLACEHOLDER LAYOUT (the window/sidebar/tabs session replaces it):
//   unified toolbar (~52 pt) · sidebar (200 pt) | panes: path bar strip,
//   list area, status line. One pane per core pane, side by side.
// The core owns windows, panes and tabs; this only mirrors `WindowModel`.

import AppKit
import SwiftUI

@MainActor
final class MainWindowController: NSWindowController, NSWindowDelegate, CoreWindowIdentifying {
    let coreWindowID: WindowID
    private unowned let model: AppModel
    private let windowModel: WindowModel
    private var panes: [PaneViewController] = []
    private let paneSplit = NSSplitView()
    private var observer: ObserverToken?
    private var closingFromCore = false

    init(model: AppModel, window wm: WindowModel) {
        self.model = model
        self.windowModel = wm
        coreWindowID = wm.id
        let w = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1200, height: 760),
                         styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView],
                         backing: .buffered, defer: false)
        w.identifier = NSUserInterfaceItemIdentifier("main-\(wm.id)")
        w.titleVisibility = .hidden
        w.toolbarStyle = .unified
        w.minSize = NSSize(width: 640, height: 400)
        super.init(window: w)
        w.delegate = self
        let tb = NSToolbar(identifier: "main")
        tb.displayMode = .iconOnly
        w.toolbar = tb
        buildContent()
        observer = model.observe { [weak self] e in
            if case .tabsChanged = e { self?.syncPanes() }
            if case .listingChanged = e { self?.updateTitle() }
        }
        // Setting the content controller sizes the window to its fitting size;
        // put it back, then let a saved frame win.
        w.setContentSize(NSSize(width: 1200, height: 760))
        w.center()
        // Scripted runs (debug automation) always start at the default size.
        if UserDefaults.standard.string(forKey: "FSDebugScript") == nil {
            w.setFrameAutosaveName("FileStudioMain-\(wm.id)")
        }
    }

    override func showWindow(_ sender: Any?) {
        super.showWindow(sender)
        if let t = activePane?.list.table { window?.makeFirstResponder(t) }
        if window?.isKeyWindow == true || model.keyWindowID == nil { model.keyWindowID = coreWindowID }
    }

    required init?(coder: NSCoder) { fatalError() }

    private func buildContent() {
        let split = NSSplitViewController()
        let sidebar = NSSplitViewItem(sidebarWithViewController: NSHostingController(
            rootView: SidebarPlaceholderView(model: model, window: windowModel)))
        sidebar.minimumThickness = 160
        sidebar.maximumThickness = 320
        split.addSplitViewItem(sidebar)

        let content = NSViewController()
        paneSplit.isVertical = true
        paneSplit.dividerStyle = .thin
        content.view = paneSplit
        let item = NSSplitViewItem(viewController: content)
        item.minimumThickness = 400
        split.addSplitViewItem(item)
        window?.contentViewController = split
        split.splitView.setPosition(200, ofDividerAt: 0)
        syncPanes()
    }

    /// One pane view per core pane.
    private func syncPanes() {
        guard let state = model.windowModel(coreWindowID)?.state else { return }
        let ids = state.panes.map(\.id)
        if panes.map(\.paneID) != ids {
            panes.forEach { $0.view.removeFromSuperview() }
            panes = ids.map { PaneViewController(model: model, windowID: coreWindowID, paneID: $0) }
            for p in panes { paneSplit.addArrangedSubview(p.view) }
            paneSplit.adjustSubviews()
        }
        for p in panes { p.sync() }
        updateTitle()
    }

    private func updateTitle() {
        guard let t = model.windowModel(coreWindowID)?.activeTabID.flatMap({ model.tabModel($0) }) else { return }
        window?.title = t.title
    }

    /// The core closed this window.
    func closeFromCore() {
        closingFromCore = true
        close()
    }

    func windowDidBecomeKey(_ notification: Notification) { model.keyWindowID = coreWindowID }

    func windowWillClose(_ notification: Notification) {
        if !closingFromCore { try? model.engine.closeWindow(coreWindowID) }
    }

    /// The focused pane's list, for debug automation and the first responder.
    var activePane: PaneViewController? {
        let i = model.windowModel(coreWindowID)?.state.activePane ?? 0
        return panes.indices.contains(i) ? panes[i] : panes.first
    }
    var allPanes: [PaneViewController] { panes }
}

/// One pane: path bar strip, list area, status line (placeholders).
@MainActor
final class PaneViewController: NSViewController {
    let paneID: PaneID
    let windowID: WindowID
    private unowned let model: AppModel
    let pathBar: PathBarPlaceholderView
    let list: ListPlaceholderView
    private let status: NSHostingView<StatusLinePlaceholderView>
    private(set) var tabID: TabID?

    init(model: AppModel, windowID: WindowID, paneID: PaneID) {
        self.model = model; self.windowID = windowID; self.paneID = paneID
        pathBar = PathBarPlaceholderView(model: model, paneID: paneID)
        list = ListPlaceholderView(model: model)
        status = NSHostingView(rootView: StatusLinePlaceholderView(model: model, tab: nil))
        super.init(nibName: nil, bundle: nil)
    }
    required init?(coder: NSCoder) { fatalError() }

    override func loadView() {
        let stack = NSStackView(views: [pathBar, list.scrollView, status])
        stack.orientation = .vertical
        stack.spacing = 0
        stack.alignment = .leading
        stack.distribution = .fill
        for v in stack.arrangedSubviews { v.translatesAutoresizingMaskIntoConstraints = false }
        NSLayoutConstraint.activate([
            pathBar.heightAnchor.constraint(equalToConstant: 36),
            pathBar.widthAnchor.constraint(equalTo: stack.widthAnchor),
            list.scrollView.widthAnchor.constraint(equalTo: stack.widthAnchor),
            status.heightAnchor.constraint(equalToConstant: 22),
            status.widthAnchor.constraint(equalTo: stack.widthAnchor),
        ])
        list.scrollView.setContentHuggingPriority(.defaultLow, for: .vertical)
        view = stack
    }

    /// Shows the pane's active tab.
    func sync() {
        guard let pane = model.windowModel(windowID)?.state.panes.first(where: { $0.id == paneID }),
              let t = pane.activeTabID, let tm = model.tabModel(t) else { return }
        if t != tabID {
            tabID = t
            pathBar.show(tm)
            list.show(tm)
            status.rootView = StatusLinePlaceholderView(model: model, tab: tm)
        }
    }
}

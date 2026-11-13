// MainWindowController.swift — one AppKit window per core window (F-002).
//
// Layout: unified toolbar (~52 pt, WindowToolbar) · sidebar (200 pt,
// `.sidebar` material, Sidebar/) | one or two panes (PaneSplitView), each a
// PaneViewController (tab bar, header with the address bar, list or columns,
// status line). The core owns windows, panes, tabs and the split; this
// mirrors `WindowModel` and sends every change back through the engine.
//
// Also here: clicking or focusing a pane makes it active in the core; mouse
// buttons 4/5 go back/forward; window frames are remembered per core window.

import AppKit
import SwiftUI

@MainActor
final class MainWindowController: NSWindowController, NSWindowDelegate, CoreWindowIdentifying {
    let coreWindowID: WindowID
    private unowned let model: AppModel
    let windowModel: WindowModel
    private let split = NSSplitViewController()
    private let panesView = PaneSplitView()
    private var panes: [PaneViewController] = []
    private var toolbarController: WindowToolbar!
    private(set) var sidebar: SidebarViewController!
    private var observer: ObserverToken?
    private var closingFromCore = false
    private var mouseMonitor: Any?
    private var responderObservation: NSKeyValueObservation?
    private var handlers: [Int] = []
    /// The pane the first responder was last seen in (to report focus moves).
    private var lastFocusPane: Int?
    private var lastActivePane: Int?

    init(model: AppModel, window wm: WindowModel) {
        self.model = model
        self.windowModel = wm
        coreWindowID = wm.id
        let w = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1200, height: 760),
                         styleMask: [.titled, .closable, .miniaturizable, .resizable, .fullSizeContentView],
                         backing: .buffered, defer: false)
        w.identifier = NSUserInterfaceItemIdentifier("main-\(wm.id)")
        w.titleVisibility = .visible
        w.toolbarStyle = .unified
        w.minSize = NSSize(width: 720, height: 420)
        w.tabbingMode = .disallowed
        w.isRestorable = false
        super.init(window: w)
        w.delegate = self
        toolbarController = WindowToolbar(model: model, window: wm)
        w.toolbar = toolbarController.toolbar
        toolbarController.addWorkspaceSwitcherIfNew()
        buildContent()
        observer = model.observe { [weak self] e in self?.event(e) }
        installHandlers()
        installMouseMonitor()
        responderObservation = w.observe(\.firstResponder, options: [.new]) { [weak self] _, _ in
            DispatchQueue.main.async { MainActor.assumeIsolated { self?.firstResponderChanged() } }
        }
        // Setting the content controller sizes the window to its fitting size;
        // put it back, then let a saved frame win.
        w.setContentSize(NSSize(width: 1200, height: 760))
        w.center()
        // Scripted runs (debug automation) always start at the default size.
        if UserDefaults.standard.string(forKey: "FSDebugScript") == nil {
            w.setFrameAutosaveName("FileStudioMain-\(wm.id)")
        }
        #if DEBUG
        WindowDebugSteps.install()
        SafetyDebugSteps.install()
        ListDropProbe.install()
        #endif
    }

    required init?(coder: NSCoder) { fatalError() }

    isolated deinit {
        if let m = mouseMonitor { NSEvent.removeMonitor(m) }
        handlers.forEach { model.unregisterShellHandler($0) }
    }

    override func showWindow(_ sender: Any?) {
        super.showWindow(sender)
        if let p = activePane { window?.makeFirstResponder(p.focusView) }
        if window?.isKeyWindow == true || model.keyWindowID == nil { model.keyWindowID = coreWindowID }
        SetupController.presentIfNeeded(model: model, on: window)
    }

    private func buildContent() {
        sidebar = SidebarViewController(model: model, window: windowModel)
        let sideItem = NSSplitViewItem(sidebarWithViewController: sidebar)
        sideItem.minimumThickness = 160
        sideItem.maximumThickness = 320
        sideItem.canCollapse = true
        sideItem.allowsFullHeightLayout = true
        split.addSplitViewItem(sideItem)

        let content = NSViewController()
        let container = NSView()
        panesView.translatesAutoresizingMaskIntoConstraints = false
        container.addSubview(panesView)
        NSLayoutConstraint.activate([
            panesView.topAnchor.constraint(equalTo: container.safeAreaLayoutGuide.topAnchor),
            panesView.bottomAnchor.constraint(equalTo: container.bottomAnchor),
            panesView.leadingAnchor.constraint(equalTo: container.leadingAnchor),
            panesView.trailingAnchor.constraint(equalTo: container.trailingAnchor),
        ])
        content.view = container
        let item = NSSplitViewItem(viewController: content)
        item.minimumThickness = 460
        split.addSplitViewItem(item)
        window?.contentViewController = split
        migrateSidebarWidth()
        applySidebar()
        NotificationCenter.default.addObserver(forName: NSSplitView.didResizeSubviewsNotification,
                                               object: split.splitView, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.sidebarResized() }
        }
        panesView.onRatioChanged = { [weak self] r in
            guard let self else { return }
            do {
                try model.engine.setLayout(window: coreWindowID, orientation: windowModel.state.orientation, splitRatio: r)
            } catch { model.statusMessage = error.localizedDescription }
        }
        panesView.onEqualize = { [weak self] in self?.run("pane.equalize") }
        syncPanes()
    }

    // MARK: Mirroring the core

    private func event(_ e: CoreEvent) {
        switch e {
        case .tabsChanged:
            syncPanes()
        case .listingChanged(let t, _, _):
            panes.first { $0.tabID == t }?.listingChanged()
            if t == windowModel.activeTabID { updateTitle(); toolbarController.refresh() }
        case .selectionChanged(let t, _, _):
            if t == windowModel.activeTabID { toolbarController.refresh() }
        case .volumesChanged:
            for p in panes { p.listingChanged() }
        case .settingsChanged:
            syncPanes()
        case .workspacesChanged:
            toolbarController.workspacesChanged()
        default:
            break
        }
    }

    /// One pane controller per core pane; reuses the ones that stay.
    private func syncPanes() {
        guard let state = model.windowModel(coreWindowID)?.state else { return }
        let ids = state.panes.map(\.id)
        if panes.map(\.paneID) != ids {
            let old = panes
            panes = ids.map { id in
                old.first { $0.paneID == id } ?? PaneViewController(model: model, window: windowModel, paneID: id)
            }
            panesView.setPanes(panes.map(\.view))
        }
        panesView.apply(orientation: state.orientation, ratio: state.splitRatio)
        let dual = panes.count == 2
        for (i, p) in panes.enumerated() { p.sync(isActive: i == state.activePane, isDual: dual) }
        // The core moved the active pane (⇥, ⌘\, a tab activated): follow it with focus.
        if lastActivePane != state.activePane {
            lastActivePane = state.activePane
            if let p = activePane, let w = window, focusedPaneIndex != state.activePane, w.firstResponder !== toolbarController.filterField.currentEditor() {
                w.makeFirstResponder(p.focusView)
            }
        }
        lastFocusPane = focusedPaneIndex
        applySidebar()
        rebuildKeyViewLoop()
        updateTitle()
        toolbarController.refresh()
        sidebar.locationChanged()
    }

    // MARK: Sidebar width (kept per window, per workspace, by the core)

    /// True while the sidebar is being set from the core, so it isn't reported back.
    private var applyingSidebar = false
    private var sidebarItem: NSSplitViewItem? { split.splitViewItems.first }

    /// Makes the sidebar match the core's width and collapsed state.
    private func applySidebar() {
        guard let item = sidebarItem else { return }
        let st = windowModel.state
        applyingSidebar = true
        defer { applyingSidebar = false }
        if item.isCollapsed != st.sidebarHidden { item.isCollapsed = st.sidebarHidden }
        if !st.sidebarHidden, abs(item.viewController.view.frame.width - st.sidebarWidth) >= 1 {
            split.splitView.setPosition(st.sidebarWidth, ofDividerAt: 0)
        }
    }

    /// The user dragged the divider or collapsed the sidebar.
    private func sidebarResized() {
        guard !applyingSidebar, let item = sidebarItem, window?.isVisible == true else { return }
        let width = item.isCollapsed ? windowModel.state.sidebarWidth : Double(item.viewController.view.frame.width)
        guard width >= 1 else { return }
        if abs(width - windowModel.state.sidebarWidth) < 1 && item.isCollapsed == windowModel.state.sidebarHidden { return }
        try? model.engine.setSidebar(window: coreWindowID, width: width, hidden: item.isCollapsed)
    }

    /// Before workspaces the split view remembered the sidebar width itself;
    /// hand that width to the core once.
    private func migrateSidebarWidth() {
        let old = "NSSplitView Subview Frames FileStudioSidebar"
        let d = UserDefaults.standard
        guard d.string(forKey: "FSDebugScript") == nil, let frames = d.array(forKey: old) as? [String],
              let first = frames.first else { return }
        d.removeObject(forKey: old)
        // "x, y, width, height, collapsed, hidden"
        let parts = first.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }
        guard parts.count >= 4, let w = Double(parts[2]), w > 0 else { return }
        try? model.engine.setSidebar(window: coreWindowID, width: w, hidden: windowModel.state.sidebarHidden)
    }

    private func updateTitle() {
        guard let t = windowModel.activeTabID.flatMap({ model.tabModel($0) }) else { return }
        if window?.title != t.title { window?.title = t.title }
        window?.representedURL = nil
    }

    // MARK: Keyboard reach

    /// Tab / ⇧Tab order: sidebar → each pane's address bar and list → the
    /// sidebar filter → back to the sidebar. In the list the core decides
    /// first what Tab means (it switches panes in dual-pane mode).
    private func rebuildKeyViewLoop() {
        var chain: [NSView] = [sidebar.outline]
        for p in panes {
            if let field = Self.firstKeyView(in: p.pathBar) { chain.append(field) }
            chain.append(p.focusView)
        }
        chain.append(sidebar.filterField)
        for (a, b) in zip(chain, chain.dropFirst() + [chain[0]]) where a.nextKeyView !== b { a.nextKeyView = b }
        window?.initialFirstResponder = activePane?.focusView
    }

    /// The first view inside `v` that can take keyboard focus.
    private static func firstKeyView(in v: NSView) -> NSView? {
        if v.acceptsFirstResponder && v.canBecomeKeyView { return v }
        for sub in v.subviews { if let k = firstKeyView(in: sub) { return k } }
        return nil
    }

    // MARK: Focus and clicks → the core's active pane

    /// The pane holding the first responder, if any.
    private var focusedPaneIndex: Int? {
        guard let v = window?.firstResponder as? NSView else { return nil }
        let owner = (v as? NSTextView).flatMap { $0.isFieldEditor ? $0.delegate as? NSView : nil } ?? v
        return panes.firstIndex { owner.isDescendant(of: $0.view) }
    }

    private func focusPane(_ i: Int) {
        guard i != windowModel.state.activePane, panes.indices.contains(i) else { return }
        do { try model.engine.focusPane(window: coreWindowID, index: UInt32(i)) } catch {
            model.statusMessage = error.localizedDescription
        }
    }

    /// Keyboard focus moved into another pane (⇥, a click in its list or address bar).
    private func firstResponderChanged() {
        let i = focusedPaneIndex
        if i != lastFocusPane {
            lastFocusPane = i
            if let i { focusPane(i) }
        }
    }

    /// Clicks anywhere in a pane activate it; mouse buttons 4/5 go back/forward.
    private func installMouseMonitor() {
        mouseMonitor = NSEvent.addLocalMonitorForEvents(matching: [.leftMouseDown, .rightMouseDown, .otherMouseDown]) {
            [weak self] e in
            // Mouse buttons 4/5 may arrive without a window; they then act on the key window.
            guard let self, e.window === self.window || (e.window == nil && self.window?.isKeyWindow == true) else { return e }
            return self.mouse(e) ? nil : e
        }
    }

    /// Returns true when the event was used up.
    private func mouse(_ e: NSEvent) -> Bool {
        if e.type == .otherMouseDown && (e.buttonNumber == 3 || e.buttonNumber == 4) {
            run(e.buttonNumber == 3 ? "nav.back" : "nav.forward")
            return true
        }
        if let i = paneIndex(at: e.locationInWindow) { focusPane(i) }
        return false
    }

    private func paneIndex(at p: NSPoint) -> Int? {
        panes.firstIndex { $0.view.bounds.contains($0.view.convert(p, from: nil)) }
    }

    private func run(_ command: String, arg: CommandArg? = nil) {
        model.perform(Invocation(id: command, arg: arg, tab: windowModel.activeTabID), via: .mouse, window: coreWindowID)
    }

    // MARK: Shell requests this window carries out

    private func installHandlers() {
        handlers.append(model.registerShellHandler(for: "focusList") { [weak self] _, ctx in
            guard let self, ctx.window == nil || ctx.window == self.coreWindowID, let p = self.paneFor(ctx.tab) else { return false }
            self.window?.makeFirstResponder(p.focusView)
            return true
        })
        handlers.append(model.registerShellHandler(for: "focusFilter") { [weak self] _, ctx in
            guard let self, ctx.window == nil || ctx.window == self.coreWindowID else { return false }
            self.toolbarController.refresh()
            self.window?.makeFirstResponder(self.toolbarController.filterField)
            return true
        })
        handlers.append(model.registerShellHandler(for: "toggleSidebar") { [weak self] _, ctx in
            guard let self, ctx.window == nil || ctx.window == self.coreWindowID else { return false }
            self.split.toggleSidebar(nil)
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.4) { MainActor.assumeIsolated { self.sidebarResized() } }
            return true
        })
    }

    private func paneFor(_ tab: TabID?) -> PaneViewController? {
        guard let tab else { return activePane }
        let idx = windowModel.state.panes.firstIndex { $0.tabs.contains { $0.id == tab } }
        return idx.flatMap { panes.indices.contains($0) ? panes[$0] : nil } ?? activePane
    }

    // MARK: Window lifecycle

    /// The core closed this window.
    func closeFromCore() {
        closingFromCore = true
        close()
    }

    func windowDidBecomeKey(_ notification: Notification) { model.keyWindowID = coreWindowID }

    func windowWillClose(_ notification: Notification) {
        if !closingFromCore { try? model.engine.closeWindow(coreWindowID) }
    }

    /// The focused pane (for debug automation and the first responder).
    var activePane: PaneViewController? {
        let i = model.windowModel(coreWindowID)?.state.activePane ?? 0
        return panes.indices.contains(i) ? panes[i] : panes.first
    }
    var allPanes: [PaneViewController] { panes }

    /// Debug: the window's layout as JSON-able values.
    var debugInfo: [String: Any] {
        ["window": Int(coreWindowID), "split": panesView.debugDescriptionLine, "activePane": windowModel.state.activePane,
         "orientation": "\(windowModel.state.orientation)", "ratio": windowModel.state.splitRatio,
         "sidebarWidth": Int(split.splitViewItems.first?.viewController.view.frame.width ?? 0),
         "panes": panes.map(\.debugInfo), "title": window?.title ?? "",
         "toolbar": window?.toolbar?.items.map(\.itemIdentifier.rawValue) ?? [],
         "toolbarHeight": Int((window?.frame.height ?? 0) - (window?.contentLayoutRect.height ?? 0))]
    }

    /// Debug: runs a mouse event through the same handler the event monitor uses
    /// (synthetic button 4/5 events never reach local monitors).
    func debugMouse(_ e: NSEvent) { _ = mouse(e) }

    /// Debug: sets the split position directly, as a finished drag would.
    func debugSetRatio(_ r: Double) { panesView.onRatioChanged?(r) }
}

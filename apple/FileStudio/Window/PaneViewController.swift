// PaneViewController.swift — one pane (F-019): its tab bar (shown from two
// tabs, or always as a setting), a 36 pt header (location glyph, the address
// bar, the "N items, X GB available" subtitle), the list or column view by
// the tab's view mode, and the status line. The active pane gets a 2 pt
// accent outline and a 3 pt accent bar on its header (dual pane only; a
// single pane is always the active one). Mirrors the core; owns no state.

import AppKit
import SwiftUI

@MainActor
final class PaneViewController: NSViewController {
    let paneID: PaneID
    let windowID: WindowID
    private unowned let model: AppModel
    private let windowModel: WindowModel
    /// The address bar (PathBar/ session's view).
    let pathBar: PathBarPlaceholderView
    /// The list view (List/ session's view).
    let list: ListPlaceholderView
    /// The column view, created on first use (Columns/ session's view).
    private var columns: NSView?
    private let state = PaneViewState()
    private let tabBar: NSHostingView<PaneTabBar>
    private let header = PaneHeaderView()
    private let content = NSView()
    private let status: NSHostingView<PaneStatusLine>
    private let outline = PaneOutlineView()
    private var tabBarHeight: NSLayoutConstraint!
    private var shownContent: NSView?
    private var shownMode: ViewMode?
    private(set) var tabID: TabID?

    init(model: AppModel, window: WindowModel, paneID: PaneID) {
        self.model = model
        self.windowModel = window
        self.windowID = window.id
        self.paneID = paneID
        pathBar = PathBarPlaceholderView(model: model, paneID: paneID)
        list = ListPlaceholderView(model: model)
        tabBar = NSHostingView(rootView: PaneTabBar(model: model, window: window, paneID: paneID))
        status = NSHostingView(rootView: PaneStatusLine(model: model, state: state))
        super.init(nibName: nil, bundle: nil)
    }
    required init?(coder: NSCoder) { fatalError() }

    override func loadView() {
        let root = PaneRootView()
        root.paneID = paneID
        root.setAccessibilityElement(true)
        root.setAccessibilityRole(.group)
        for v in [tabBar, header, content, status] as [NSView] {
            v.translatesAutoresizingMaskIntoConstraints = false
            root.addSubview(v)
        }
        header.install(pathBar: pathBar)
        outline.translatesAutoresizingMaskIntoConstraints = false
        root.addSubview(outline)
        tabBarHeight = tabBar.heightAnchor.constraint(equalToConstant: 0)
        tabBar.sizingOptions = []
        status.sizingOptions = []
        NSLayoutConstraint.activate([
            tabBar.topAnchor.constraint(equalTo: root.topAnchor),
            tabBar.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            tabBar.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            tabBarHeight,
            header.topAnchor.constraint(equalTo: tabBar.bottomAnchor),
            header.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            header.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            header.heightAnchor.constraint(equalToConstant: PaneHeaderView.height),
            content.topAnchor.constraint(equalTo: header.bottomAnchor),
            content.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            content.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            status.topAnchor.constraint(equalTo: content.bottomAnchor),
            status.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            status.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            status.bottomAnchor.constraint(equalTo: root.bottomAnchor),
            status.heightAnchor.constraint(equalToConstant: PaneStatusLine.height),
            outline.topAnchor.constraint(equalTo: root.topAnchor),
            outline.bottomAnchor.constraint(equalTo: root.bottomAnchor),
            outline.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            outline.trailingAnchor.constraint(equalTo: root.trailingAnchor),
        ])
        view = root
    }

    /// The pane's state in the core's window.
    private var pane: PaneState? { windowModel.state.panes.first { $0.id == paneID } }

    /// Shows the pane's active tab, its view mode, the tab bar and the active marks.
    func sync(isActive: Bool, isDual: Bool) {
        _ = view
        guard let pane, let t = pane.activeTabID, let tm = model.tabModel(t) else { return }
        if t != tabID {
            tabID = t
            pathBar.show(tm)
            state.tab = tm
            shownMode = nil
        }
        syncContent()
        let showTabs = pane.tabs.count >= 2 || WindowPreferences.tabBarAlways(model)
        let h = showTabs ? PaneTabBar.height : 0
        if tabBarHeight.constant != h { tabBarHeight.constant = h }
        tabBar.isHidden = !showTabs
        let marked = isActive && isDual
        state.isActive = isActive
        state.isDual = isDual
        header.setActive(marked)
        outline.isHidden = !marked
        header.update(tm, volumes: model.volumes)
        view.setAccessibilityLabel(isDual ? (isActive ? "Active pane, \(tm.title)" : "Pane, \(tm.title)") : tm.title)
    }

    /// Re-reads the subtitle and view mode after a listing change.
    func listingChanged() {
        guard let tm = state.tab else { return }
        header.update(tm, volumes: model.volumes)
        syncContent()
    }

    /// Puts the list or the column view in the content area, by view mode.
    private func syncContent() {
        guard let tm = state.tab else { return }
        let mode = tm.api.view().mode
        if mode == shownMode, shownContent != nil { return }
        shownMode = mode
        let wanted: PaneContent = mode == .columns ? (columnsContent ?? list) : list
        let v = wanted.contentView
        if shownContent !== v {
            let hadFocus = shownContent.map { isFocus(in: $0) } ?? false
            shownContent?.removeFromSuperview()
            v.translatesAutoresizingMaskIntoConstraints = false
            content.addSubview(v)
            NSLayoutConstraint.activate([
                v.topAnchor.constraint(equalTo: content.topAnchor),
                v.bottomAnchor.constraint(equalTo: content.bottomAnchor),
                v.leadingAnchor.constraint(equalTo: content.leadingAnchor),
                v.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            ])
            shownContent = v
            if hadFocus { view.window?.makeFirstResponder(wanted.focusView) }
        }
        wanted.show(tm)
    }

    /// The list view switches itself into column (Miller) view by the tab's
    /// view mode (List/ListPlaceholderView), so the pane always shows the list.
    private var columnsContent: PaneContent? { nil }

    /// The view that takes keyboard focus for this pane.
    var focusView: NSView {
        (shownMode == .columns ? columnsContent : nil)?.focusView ?? list.table
    }

    /// Is the first responder inside `v`?
    private func isFocus(in v: NSView) -> Bool {
        guard let r = view.window?.firstResponder as? NSView else { return false }
        return r.isDescendant(of: v)
    }

    /// Debug: what the pane shows.
    var debugInfo: [String: Any] {
        ["pane": Int(paneID), "tab": tabID.map { Int($0) } ?? -1, "tabBar": !tabBar.isHidden,
         "tabs": pane?.tabs.map { model.tabModel($0.id)?.title ?? $0.title } ?? [],
         "active": state.isActive, "outline": !outline.isHidden, "subtitle": header.subtitle.stringValue,
         "mode": "\(shownMode.map { "\($0)" } ?? "?")", "frame": NSStringFromRect(view.frame)]
    }
}

/// The pane's root view; knows its pane for hit-testing from the window.
@MainActor
final class PaneRootView: NSView {
    var paneID: PaneID = 0
    override var isFlipped: Bool { true }
}

/// 36 pt header: a location glyph, the address bar on top and the subtitle
/// underneath; a 3 pt accent bar along the top when the pane is active.
@MainActor
final class PaneHeaderView: NSView {
    static let height: CGFloat = 36
    let glyph = NSImageView()
    let subtitle = NSTextField(labelWithString: "")
    private let bar = NSView()
    private var location = ""

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        glyph.imageScaling = .scaleProportionallyUpOrDown
        glyph.setAccessibilityElement(false)
        subtitle.font = .systemFont(ofSize: 11)
        subtitle.textColor = .secondaryLabelColor
        subtitle.lineBreakMode = .byTruncatingTail
        subtitle.setAccessibilityLabel("Folder summary")
        bar.wantsLayer = true
        bar.isHidden = true
        for v in [glyph, subtitle, bar] {
            v.translatesAutoresizingMaskIntoConstraints = false
            addSubview(v)
        }
        NSLayoutConstraint.activate([
            glyph.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 10),
            glyph.centerYAnchor.constraint(equalTo: centerYAnchor, constant: 1),
            glyph.widthAnchor.constraint(equalToConstant: 24),
            glyph.heightAnchor.constraint(equalToConstant: 24),
            subtitle.leadingAnchor.constraint(equalTo: glyph.trailingAnchor, constant: 8),
            subtitle.trailingAnchor.constraint(lessThanOrEqualTo: trailingAnchor, constant: -10),
            subtitle.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -3),
            bar.topAnchor.constraint(equalTo: topAnchor),
            bar.leadingAnchor.constraint(equalTo: leadingAnchor),
            bar.trailingAnchor.constraint(equalTo: trailingAnchor),
            bar.heightAnchor.constraint(equalToConstant: 3),
        ])
    }
    required init?(coder: NSCoder) { fatalError() }

    override var isFlipped: Bool { true }

    /// Places the address bar: its text starts 12 pt inside its view, so the
    /// view starts 12 pt left of the subtitle to line the two up.
    func install(pathBar: NSView) {
        pathBar.translatesAutoresizingMaskIntoConstraints = false
        addSubview(pathBar, positioned: .below, relativeTo: bar)
        NSLayoutConstraint.activate([
            pathBar.leadingAnchor.constraint(equalTo: glyph.trailingAnchor, constant: -4),
            pathBar.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -4),
            pathBar.topAnchor.constraint(equalTo: topAnchor, constant: 2),
            pathBar.heightAnchor.constraint(equalToConstant: 20),
        ])
    }

    func update(_ tab: TabModel, volumes: [Volume]) {
        let s = PaneText.subtitle(tab, volumes: volumes)
        if subtitle.stringValue != s { subtitle.stringValue = s }
        if tab.location != location {
            location = tab.location
            glyph.image = FolderIcon.icon(for: location)
        }
    }

    func setActive(_ on: Bool) { bar.isHidden = !on }

    override func updateLayer() {
        layer?.backgroundColor = NSColor.textBackgroundColor.cgColor
        bar.layer?.backgroundColor = NSColor.controlAccentColor.cgColor
    }
    override var wantsUpdateLayer: Bool { true }

    override func draw(_ dirtyRect: NSRect) {}

    override func layout() {
        super.layout()
        needsDisplay = true
    }
}

/// The 2 pt accent outline around the active pane. Never takes clicks.
@MainActor
final class PaneOutlineView: NSView {
    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        isHidden = true
        setAccessibilityElement(false)
    }
    required init?(coder: NSCoder) { fatalError() }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }
    override var wantsUpdateLayer: Bool { true }
    override func updateLayer() {
        layer?.borderWidth = 2
        layer?.borderColor = NSColor.controlAccentColor.cgColor
        layer?.cornerRadius = 0
    }
}

/// Folder icons by path (Desktop, Downloads… get their own), cached.
@MainActor
enum FolderIcon {
    private static var cache: [String: NSImage] = [:]
    static func icon(for path: String) -> NSImage {
        if let i = cache[path] { return i }
        let i = FileManager.default.fileExists(atPath: path) ? NSWorkspace.shared.icon(forFile: path)
            : IconCache.shared.icon(key: "folder")
        if cache.count > 500 { cache.removeAll() }
        cache[path] = i
        return i
    }
}

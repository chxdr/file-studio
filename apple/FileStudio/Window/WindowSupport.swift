// WindowSupport.swift — small shared pieces of the window layer: the
// hand-shake with the list/column views, shell preferences the core doesn't
// hold yet, motion helpers and pane formatting.

import AppKit
import SwiftUI

// MARK: - Pane content hand-shake

/// What a pane shows under its header. The list and column views (List/,
/// Columns/) adopt it, so the pane can swap them by the tab's view mode.
/// `ColumnsPlaceholderView` doesn't adopt it yet; until it does, columns mode
/// is drawn by the list (as its README says).
@MainActor
protocol PaneContent: AnyObject {
    /// The view to place in the pane's content area.
    var contentView: NSView { get }
    /// The view that should take keyboard focus for this content.
    var focusView: NSView { get }
    /// Shows a tab (called when the pane's active tab changes).
    func show(_ tab: TabModel)
}

extension ListPlaceholderView: PaneContent {
    var contentView: NSView { scrollView }
    var focusView: NSView { table }
}

// MARK: - Window preferences

/// Window-layer preferences, all held by core settings.
@MainActor
enum WindowPreferences {
    /// Show the tab bar even with one tab (the default shows it from two tabs).
    static func tabBarAlways(_ m: AppModel) -> Bool { Core.bool(m.engine, "appearance.tab_bar_always") ?? false }

    /// First-run setup has been finished or skipped.
    static func setupDone(_ m: AppModel) -> Bool { Core.bool(m.engine, "general.setup_done") ?? false }

    static func setSetupDone(_ m: AppModel) { try? Core.set(m.engine, "general.setup_done", bool: true) }

    /// One-time move of the old UserDefaults stopgaps (`FSTabBarAlways`,
    /// `FSSetupDone`) into core settings, so nobody's choice resets. Only
    /// a value that was actually saved is carried over, then the old key is
    /// removed. Call once at launch, before the first window.
    static func migrateLegacyDefaults(_ m: AppModel) {
        let d = UserDefaults.standard
        if d.object(forKey: "FSTabBarAlways") != nil {
            if d.bool(forKey: "FSTabBarAlways") { try? Core.set(m.engine, "appearance.tab_bar_always", bool: true) }
            d.removeObject(forKey: "FSTabBarAlways")
        }
        if d.object(forKey: "FSSetupDone") != nil {
            if d.bool(forKey: "FSSetupDone") { try? Core.set(m.engine, "general.setup_done", bool: true) }
            d.removeObject(forKey: "FSSetupDone")
        }
    }
}

// MARK: - Motion (Import Studio's D-026)

/// Motion helpers: springs in the direction of travel, quick cross-fades,
/// and nothing but fades (or nothing) under Reduce Motion.
@MainActor
enum Motion {
    static var reduce: Bool { NSWorkspace.shared.accessibilityDisplayShouldReduceMotion }
    /// The sliding pill and reorders.
    static var pill: Animation? { reduce ? nil : .spring(duration: 0.3, bounce: 0.18) }
    /// Cross-fades between states.
    static let fade: Animation = .easeInOut(duration: 0.14)
}

// MARK: - Formatting

/// Plain-language bits shared by the pane header and status line. Counts and
/// sizes come from the core; this only spells them.
@MainActor
enum PaneText {
    static func items(_ n: UInt32) -> String { n == 1 ? "1 item" : "\(n.formatted()) items" }

    static func bytes(_ n: UInt64) -> String { ByteCountFormatter.string(fromByteCount: Int64(clamping: n), countStyle: .file) }

    /// The volume a path lives on: the one with the longest matching mount path.
    static func volume(for path: String, in volumes: [Volume]) -> Volume? {
        volumes.filter { v in
            v.mountPath == "/" || path == v.mountPath || path.hasPrefix(v.mountPath.hasSuffix("/") ? v.mountPath : v.mountPath + "/")
        }.max { $0.mountPath.count < $1.mountPath.count }
    }

    /// "12 items, 241.5 GB available" (ForkLift's pane subtitle).
    static func subtitle(_ tab: TabModel, volumes: [Volume]) -> String {
        let l = tab.listing
        if l.state == .loading && l.count == 0 { return "Loading…" }
        if let e = l.error, l.state == .error { return e }
        var s = items(l.count)
        if let v = volume(for: tab.location, in: volumes), v.totalBytes > 0 {
            s += ", \(bytes(v.freeBytes)) available"
        }
        return s
    }
}

// MARK: - Middle-click catcher for SwiftUI

/// Sits over a SwiftUI view and takes only middle clicks; every other event
/// falls through to the SwiftUI content underneath.
struct MiddleClickCatcher: NSViewRepresentable {
    let action: () -> Void

    func makeNSView(context: Context) -> CatcherView { CatcherView() }
    func updateNSView(_ v: CatcherView, context: Context) { v.action = action }

    final class CatcherView: NSView {
        var action: (() -> Void)?
        override func hitTest(_ point: NSPoint) -> NSView? {
            switch NSApp.currentEvent?.type {
            case .otherMouseDown?, .otherMouseUp?, .otherMouseDragged?: super.hitTest(point)
            default: nil
            }
        }
        override func otherMouseDown(with event: NSEvent) {
            if event.buttonNumber == 2 { action?() } else { super.otherMouseDown(with: event) }
        }
        override func otherMouseUp(with event: NSEvent) {
            if event.buttonNumber != 2 { super.otherMouseUp(with: event) }
        }
    }
}

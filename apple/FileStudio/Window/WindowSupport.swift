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

// MARK: - Preferences the core doesn't hold yet

/// Window-layer preferences with no core setting yet. Each is a shell-side
/// stopgap (UserDefaults) listed in the hand-over for the coordinator; move
/// them into core `Settings` when the core grows the fields.
@MainActor
enum WindowPreferences {
    static let changed = Notification.Name("FSWindowPreferencesChanged")

    /// Show the tab bar even with one tab (the default shows it from two tabs).
    /// The Settings session binds a toggle to this.
    static var tabBarAlways: Bool {
        get { UserDefaults.standard.bool(forKey: "FSTabBarAlways") }
        set {
            UserDefaults.standard.set(newValue, forKey: "FSTabBarAlways")
            NotificationCenter.default.post(name: changed, object: nil)
        }
    }

    /// First-run setup has been finished or skipped.
    static var setupDone: Bool {
        get { UserDefaults.standard.bool(forKey: "FSSetupDone") }
        set { UserDefaults.standard.set(newValue, forKey: "FSSetupDone") }
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

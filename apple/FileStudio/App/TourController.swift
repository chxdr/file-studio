// TourController.swift — presents the power tour (`app.power_tour`, Help ›
// File Studio Power Tour, and "Take the 1-minute tour" on setup's last page)
// as a sheet on a main window, and holds where the user is. Short and
// skippable: Esc or Close ends it at any card.

import AppKit
import SwiftUI

/// The tour's state: which card, and which way the last move went.
@MainActor
@Observable
final class TourState {
    let cards = TourCard.all
    var index = 0
    /// Last move went forward (cards slide in from the trailing edge).
    var forward = true
    /// Debug automation freezes the demos at this point of their loop (0…1)
    /// so snapshots are stable. Reduce Motion freezes them too.
    var frozenPhase: Double?

    var card: TourCard { cards[index] }
    var isLast: Bool { index == cards.count - 1 }
}

@MainActor
enum TourController {
    private static var sheet: NSWindow?
    /// The open tour's state (debug automation moves between cards).
    private(set) static var state: TourState?
    private static var installed = false

    /// Handles `ShellRequest.showPowerTour` (call once at launch).
    static func install(_ model: AppModel) {
        guard !installed else { return }
        installed = true
        model.registerShellHandler(for: "showPowerTour") { _, ctx in
            present(model: model, on: hostWindow(ctx.window))
            return true
        }
        #if DEBUG
        TourDebugSteps.install()
        #endif
    }

    /// A main window to put the sheet on: the request's, else the key one, else any.
    static func hostWindow(_ id: WindowID?) -> NSWindow? {
        let mains = NSApp.windows.filter { $0.windowController is CoreWindowIdentifying && $0.isVisible }
        if let id, let w = mains.first(where: { ($0.windowController as? CoreWindowIdentifying)?.coreWindowID == id }) {
            return w
        }
        return mains.first { $0.isKeyWindow } ?? mains.first
    }

    /// Shows the tour at a card (0-based). If it is open already, moves there.
    static func present(model: AppModel, on window: NSWindow? = nil, card: Int = 0) {
        if state != nil {
            go(to: card)
            return
        }
        guard let host = window ?? hostWindow(nil) else { return }
        // A sheet can't open over another sheet (setup's "Take the tour" ends setup first).
        if let other = host.attachedSheet {
            host.endSheet(other)
        }
        let s = TourState()
        s.index = min(max(card, 0), s.cards.count - 1)
        state = s
        let vc = NSHostingController(rootView: TourView(model: model, state: s, close: { close() }))
        vc.sizingOptions = [.preferredContentSize]
        let w = NSWindow(contentViewController: vc)
        w.identifier = NSUserInterfaceItemIdentifier("powerTour")
        w.title = "File Studio Power Tour"
        w.setAccessibilityLabel("File Studio Power Tour")
        sheet = w
        host.beginSheet(w)
    }

    /// Moves to a card (debug automation and the Back/Next buttons and arrow keys).
    static func go(to i: Int) {
        guard let s = state, s.cards.indices.contains(i), i != s.index else { return }
        s.forward = i > s.index
        withAnimation(Motion.reduce ? .easeInOut(duration: 0.15) : .spring(duration: 0.45, bounce: 0.15)) { s.index = i }
        let c = s.cards[i]
        NSAccessibility.post(element: sheet as Any, notification: .announcementRequested, userInfo: [
            .announcement: "Card \(i + 1) of \(s.cards.count): \(c.title)",
            .priority: NSAccessibilityPriorityLevel.high.rawValue,
        ])
    }

    /// Ends the tour.
    static func close() {
        if let w = sheet { w.sheetParent?.endSheet(w) }
        sheet = nil
        state = nil
    }

    /// Card 8: close the tour and open Settings › Shortcuts through the core.
    static func openShortcutSettings(model: AppModel) {
        let host = sheet?.sheetParent
        close()
        let wid = (host?.windowController as? CoreWindowIdentifying)?.coreWindowID
        model.perform(Invocation(id: "app.keyboard_shortcuts"), via: .mouse, window: wid)
    }
}

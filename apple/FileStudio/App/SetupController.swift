// SetupController.swift — presents first-run setup (F-011) as a sheet on the
// first window, and holds the setup's state. Short and skippable; choices go
// to core settings (`keyboard.profile`, `general.dualPaneByDefault`) and the
// zoxide import to the core's frecency.
//
// Hook for the Settings session: `SetupController.present(model:)` re-runs it
// on the key window ("Run Setup Again…").

import AppKit
import SwiftUI

/// The setup sheet's state: the pages on offer and where the user is.
@MainActor
@Observable
final class SetupState {
    enum Page: String, CaseIterable { case welcome, fullDiskAccess, keymap, layout, zoxide, done }

    let pages: [Page]
    var index = 0
    /// Last move went forward (pages slide in from the trailing edge).
    var forward = true
    var hasFullDiskAccess = SetupController.hasFullDiskAccess()

    init(offerZoxide: Bool) {
        pages = Page.allCases.filter { $0 != .zoxide || offerZoxide }
    }

    var page: Page { pages[index] }
    var isLast: Bool { index == pages.count - 1 }

    var titles: [String] {
        pages.map {
            switch $0 {
            case .welcome: "Welcome"
            case .fullDiskAccess: "Access"
            case .keymap: "Keys"
            case .layout: "Layout"
            case .zoxide: "History"
            case .done: "Done"
            }
        }
    }
}

@MainActor
enum SetupController {
    private static var sheet: NSWindow?
    /// The open setup's state (debug automation moves between pages).
    private(set) static var state: SetupState?
    private static var checkedThisLaunch = false

    /// Shows setup once, on the first window, until it is finished or skipped.
    /// Scripted debug runs skip it unless they ask for it (`setup:` step).
    static func presentIfNeeded(model: AppModel, on window: NSWindow?) {
        guard !checkedThisLaunch else { return }
        checkedThisLaunch = true
        guard !WindowPreferences.setupDone(model), UserDefaults.standard.string(forKey: "FSDebugScript") == nil else { return }
        // Let the window finish appearing first.
        DispatchQueue.main.async { MainActor.assumeIsolated { present(model: model, on: window) } }
    }

    /// Runs setup as a sheet on `window` (default: the key main window).
    static func present(model: AppModel, on window: NSWindow? = nil, page: Int = 0) {
        guard let host = window ?? NSApp.keyWindow ?? NSApp.mainWindow ?? NSApp.windows.first(where: {
            $0.windowController is CoreWindowIdentifying }) else { return }
        if let s = state {
            go(to: page)
            _ = s
            return
        }
        let s = SetupState(offerZoxide: zoxideDatabase(model: model) != nil)
        s.index = min(max(page, 0), s.pages.count - 1)
        state = s
        let vc = NSHostingController(rootView: SetupView(model: model, state: s, finish: { finish(model: model) }))
        vc.sizingOptions = [.preferredContentSize]
        let w = NSWindow(contentViewController: vc)
        w.identifier = NSUserInterfaceItemIdentifier("setup")
        w.title = "Set Up File Studio"
        sheet = w
        host.beginSheet(w)
    }

    /// Moves to a page (debug automation and the Back/Continue buttons).
    static func go(to i: Int) {
        guard let s = state, s.pages.indices.contains(i), i != s.index else { return }
        s.forward = i > s.index
        withAnimation(Motion.reduce ? .easeInOut(duration: 0.15) : .spring(duration: 0.45, bounce: 0.15)) { s.index = i }
    }

    static func finish(model: AppModel) {
        WindowPreferences.setSetupDone(model)
        if let w = sheet { w.sheetParent?.endSheet(w) }
        sheet = nil
        state = nil
    }

    // MARK: OS checks (Cocoa/POSIX only; the decisions stay in the core)

    /// Full Disk Access shows as being able to open the system TCC database,
    /// which only processes with Full Disk Access can read.
    static func hasFullDiskAccess() -> Bool {
        FileHandle(forReadingAtPath: "/Library/Application Support/com.apple.TCC/TCC.db") != nil
    }

    /// Opens System Settings › Privacy & Security › Full Disk Access.
    static func openFullDiskAccessSettings() {
        if let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles") {
            NSWorkspace.shared.open(url)
        }
    }

    /// zoxide's database where zoxide keeps it (`$_ZO_DATA_DIR`, else
    /// ~/Library/Application Support/zoxide), if it exists. The mock offers
    /// the page always so it can be seen.
    static func zoxideDatabase(model: AppModel) -> String? {
        if model.engine.implementationName == "Mock" { return "/Mock/zoxide/db.zo" }
        let dir = ProcessInfo.processInfo.environment["_ZO_DATA_DIR"]
            ?? (NSHomeDirectory() as NSString).appendingPathComponent("Library/Application Support/zoxide")
        let path = (dir as NSString).appendingPathComponent("db.zo")
        return FileManager.default.fileExists(atPath: path) ? path : nil
    }
}

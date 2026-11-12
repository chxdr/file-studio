// main.swift — an AppKit app (the menu bar and windows are ours, not SwiftUI's
// scene machinery); SwiftUI is used inside windows.

import AppKit

MainActor.assumeIsolated {
    let app = NSApplication.shared
    let delegate = AppDelegate()
    app.delegate = delegate
    app.setActivationPolicy(.regular)
    withExtendedLifetime(delegate) { app.run() }
}

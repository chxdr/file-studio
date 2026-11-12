// AppDelegate.swift — launch, windows, menus, quit.
//
// Launch: make the engine (EngineFactory), restore the session, open one
// window controller per core window, build the menus from the registry,
// start routing keys and volume notifications, run any debug script.

import AppKit
import SwiftUI

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private(set) var model: AppModel!
    private var keyRouter: KeyRouter!
    private var volumeWatcher: VolumeWatcher?
    private var controllers: [WindowID: MainWindowController] = [:]
    private var settingsWindow: NSWindow?

    func applicationDidFinishLaunching(_ note: Notification) {
        model = AppModel(engine: makeEngine())
        PlatformActions.install(model)
        installAppHandlers()
        model.windowsDidChange = { [weak self] in self?.syncWindows() }
        model.start()
        NSApp.mainMenu = MainMenu.build(model: model, target: self)
        keyRouter = KeyRouter(model: model)
        keyRouter.install()
        volumeWatcher = VolumeWatcher(engine: model.engine)
        syncWindows()
        NSApp.activate()
        #if DEBUG
        DebugAutomation.runIfRequested(model)
        #endif
    }

    func applicationWillTerminate(_ note: Notification) { model?.appWillQuit() }

    func applicationShouldTerminateAfterLastWindowClosed(_ app: NSApplication) -> Bool { false }

    func applicationShouldHandleReopen(_ app: NSApplication, hasVisibleWindows: Bool) -> Bool {
        if !hasVisibleWindows { _ = try? model.engine.newWindow(path: nil) }
        return true
    }

    /// Opens a controller for each core window and closes the rest.
    private func syncWindows() {
        guard let model else { return }
        for (id, w) in model.windows where controllers[id] == nil {
            let c = MainWindowController(model: model, window: w)
            controllers[id] = c
            c.showWindow(nil)
        }
        for id in controllers.keys where model.windows[id] == nil {
            controllers[id]?.closeFromCore()
            controllers[id] = nil
        }
    }

    /// Requests the app itself carries out (windows, Settings).
    private func installAppHandlers() {
        model.registerShellHandler(for: "openWindow") { [weak self] _, _ in self?.syncWindows(); return true }
        model.registerShellHandler(for: "closeWindow") { [weak self] _, _ in self?.syncWindows(); return true }
        model.registerShellHandler(for: "showSettings") { [weak self] _, _ in self?.showSettings(); return true }
    }

    func showSettings() {
        if settingsWindow == nil {
            let w = NSWindow(contentViewController: NSHostingController(rootView: SettingsPlaceholderView(model: model)))
            w.title = "Settings"
            w.identifier = NSUserInterfaceItemIdentifier("settings")
            w.isReleasedWhenClosed = false
            settingsWindow = w
        }
        settingsWindow?.makeKeyAndOrderFront(nil)
    }

    /// Menu items built from the registry land here.
    @objc func runCommand(_ sender: NSMenuItem) {
        guard let id = sender.representedObject as? String else { return }
        if NSApp.currentEvent?.type == .keyDown {
            // The core already saw this key and declined it (KeyRouter); a text
            // field may still want it as a standard edit action.
            if let sel = MainMenu.textActions[id] { NSApp.sendAction(sel, to: nil, from: sender) }
            return
        }
        let (ctx, wid) = KeyRouter.context(in: NSApp.keyWindow)
        model.perform(Invocation(id: id, tab: ctx.tab), via: .menu, window: wid)
    }
}

// PlatformActions.swift — the default handlers for Cocoa-only shell requests
// (F-021): open, Open With, editor, Quick Look, Get Info, reveal, Terminal,
// pasteboard, eject, drive info. UI sessions register their own handlers for
// the requests that need views (rename field, address bar, sheets); newer
// handlers run first, so they can also override these.

import AppKit

@MainActor
enum PlatformActions {
    static func install(_ model: AppModel) {
        func on(_ kind: String, _ fn: @escaping (ShellRequest) -> Void) {
            model.registerShellHandler(for: kind) { req, _ in fn(req); return true }
        }
        on("open") { if case .open(let p) = $0 { open(p) } }
        on("openWith") { if case .openWith(let p) = $0 { chooseAppAndOpen(p) } }
        on("openInEditor") { if case .openInEditor(let p) = $0 { openInEditor(p) } }
        on("quickLook") { if case .quickLook(let p) = $0 { QuickLookController.shared.toggle(p) } }
        on("getInfo") { if case .getInfo(let p) = $0 { getInfo(p) } }
        on("revealInFinder") { if case .revealInFinder(let p) = $0 { reveal(p) } }
        on("openTerminal") { if case .openTerminal(let d) = $0 { openTerminal(d, app: model.settings.general.terminalApp) } }
        on("copyText") {
            if case .copyText(let t) = $0 {
                Pasteboard.writeText(t)
                model.statusMessage = t.contains("\n") ? "Copied paths" : "Copied “\(t)”"
            }
        }
        on("writeFilePasteboard") { if case .writeFilePasteboard(let p) = $0 { Pasteboard.writeFiles(p) } }
        on("eject") { if case .eject(let p) = $0 { eject(p, model: model) } }
        on("showDriveInfo") { if case .showDriveInfo(let p) = $0 { getInfo([p]) } }
        on("toggleSidebar") { _ in NSApp.sendAction(#selector(NSSplitViewController.toggleSidebar(_:)), to: nil, from: nil) }
    }

    // MARK: NSWorkspace

    static func open(_ paths: [String]) {
        for p in paths { NSWorkspace.shared.open(URL(fileURLWithPath: p)) }
    }

    /// Lets the user pick an app, then opens the items with it.
    static func chooseAppAndOpen(_ paths: [String]) {
        let panel = NSOpenPanel()
        panel.directoryURL = URL(fileURLWithPath: "/Applications")
        panel.allowedContentTypes = [.application]
        panel.prompt = "Open"
        guard panel.runModal() == .OK, let app = panel.url else { return }
        open(paths, with: app)
    }

    /// Opens items with a given app (the Open With submenu).
    static func open(_ paths: [String], with app: URL) {
        NSWorkspace.shared.open(paths.map { URL(fileURLWithPath: $0) }, withApplicationAt: app,
                                configuration: NSWorkspace.OpenConfiguration())
    }

    /// The user's default editor: the app that opens plain text.
    static func openInEditor(_ paths: [String]) {
        guard let editor = NSWorkspace.shared.urlForApplication(toOpen: .plainText) else { return open(paths) }
        open(paths, with: editor)
    }

    static func reveal(_ paths: [String]) {
        NSWorkspace.shared.activateFileViewerSelecting(paths.map { URL(fileURLWithPath: $0) })
    }

    /// Finder's Get Info window (until File Studio has its own inspector).
    static func getInfo(_ paths: [String]) {
        let list = paths.map { "POSIX file \"\($0.replacingOccurrences(of: "\"", with: "\\\""))\"" }.joined(separator: ", ")
        let src = "tell application \"Finder\"\nactivate\nopen information window of (every item of {\(list)})\nend tell"
        var err: NSDictionary?
        NSAppleScript(source: src)?.executeAndReturnError(&err)
        if err != nil { reveal(paths) }
    }

    /// Opens a folder in the chosen terminal; falls back to Terminal.app.
    static func openTerminal(_ dir: String, app: TerminalApp) {
        let url = URL(fileURLWithPath: dir, isDirectory: true)
        let ws = NSWorkspace.shared
        let appURL = ws.urlForApplication(withBundleIdentifier: app.bundleID)
            ?? ws.urlForApplication(withBundleIdentifier: TerminalApp.terminal.bundleID)
        guard let appURL else { return }
        ws.open([url], withApplicationAt: appURL, configuration: NSWorkspace.OpenConfiguration())
    }

    static func eject(_ path: String, model: AppModel) {
        do {
            try NSWorkspace.shared.unmountAndEjectDevice(at: URL(fileURLWithPath: path))
        } catch {
            model.statusMessage = "Couldn't eject: \(error.localizedDescription)"
        }
    }
}

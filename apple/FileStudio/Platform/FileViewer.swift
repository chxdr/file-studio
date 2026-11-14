// FileViewer.swift — File Studio as the Mac's default file viewer.
//
// - `DefaultFileViewer` reads and writes the two system settings: the global
//   `NSFileViewer` preference (where `activateFileViewerSelectingURLs`, so
//   every app's "Show in Finder" and `open -R`, goes) and the `public.folder`
//   handler (where `open <folder>` goes; macOS 26+ keeps that for Finder).
//   The core (`file_viewer_status`) says what the pair means.
// - `SystemOpen` receives what other apps send. `NSWorkspace`'s reveal
//   (`activateFileViewerSelectingURLs`, `open -R`) reaches the `NSFileViewer`
//   app as a plain open-documents event, so it arrives in
//   `application(_:open:)`: a file shows its folder with it selected; a
//   folder opens (the event is the same as `open -a "File Studio" <folder>`,
//   so a revealed folder can't be told apart). Scripts that send Finder's
//   own reveal event (`misc`/`mvis`) are handled too. The core decides which
//   folders to show and what to select (`openFromSystem`).
// - `SystemOpen.revealInFinder` is File Studio's own Reveal in Finder. While
//   File Studio is the viewer the system call would come straight back here,
//   so it asks Finder itself.

import AppKit
import Carbon.HIToolbox
import UniformTypeIdentifiers

@MainActor
enum DefaultFileViewer {
    static let finderID = "com.apple.finder"
    private static let key = "NSFileViewer" as CFString

    /// This app's bundle id (Release and Debug differ, so each only ever
    /// claims itself).
    static var ownID: String { Bundle.main.bundleIdentifier ?? "app.filestudio.mac" }

    /// The global `NSFileViewer` preference, if set.
    static func fileViewer() -> String? {
        CFPreferencesAppSynchronize(kCFPreferencesAnyApplication)
        return CFPreferencesCopyValue(key, kCFPreferencesAnyApplication, kCFPreferencesCurrentUser,
                                      kCFPreferencesAnyHost) as? String
    }

    /// The bundle id of the app that opens folders.
    static func folderHandler() -> String? {
        NSWorkspace.shared.urlForApplication(toOpen: UTType.folder).flatMap { Bundle(url: $0)?.bundleIdentifier }
    }

    /// The current state, worked out by the core.
    static func status() -> FfiViewerStatus {
        fileViewerStatus(fileViewer: fileViewer(), folderHandler: folderHandler(), ownId: ownID)
    }

    /// Makes this app the viewer: every app's "Show in Finder" comes here.
    /// Also asks for folders; macOS 26+ refuses that for every app but
    /// Finder (paramErr), which is expected and not an error here.
    static func makeDefault(_ done: @escaping @MainActor (Error?) -> Void) {
        setFileViewer(ownID)
        setFolderHandler(Bundle.main.bundleURL) { _ in done(nil) }
    }

    /// Gives "Show in Finder" (and folders, if this app had them) back to Finder.
    static func restoreFinder(_ done: @escaping @MainActor (Error?) -> Void) {
        setFileViewer(nil)
        guard folderHandler().map({ $0.caseInsensitiveCompare(finderID) != .orderedSame }) ?? false,
              let finder = NSWorkspace.shared.urlForApplication(withBundleIdentifier: finderID) else {
            return done(nil)
        }
        setFolderHandler(finder, done)
    }

    private static func setFileViewer(_ id: String?) {
        CFPreferencesSetValue(key, id as CFString?, kCFPreferencesAnyApplication, kCFPreferencesCurrentUser,
                              kCFPreferencesAnyHost)
        CFPreferencesSynchronize(kCFPreferencesAnyApplication, kCFPreferencesCurrentUser, kCFPreferencesAnyHost)
    }

    private static func setFolderHandler(_ app: URL, _ done: @escaping @MainActor (Error?) -> Void) {
        NSWorkspace.shared.setDefaultApplication(at: app, toOpen: UTType.folder) { error in
            if let error { print("SYSOPEN folder handler not changed: \(error)") }
            DispatchQueue.main.async { MainActor.assumeIsolated { done(error) } }
        }
    }
}

@MainActor
enum SystemOpen {
    private static weak var model: AppModel?
    /// Requests that arrived before the engine was ready (a launch by a folder open).
    private static var pending: [(paths: [String], reveal: Bool)] = []

    /// Catches reveal requests. Call from `applicationWillFinishLaunching`,
    /// so one that launched the app isn't missed.
    static func installEarly() {
        NSAppleEventManager.shared().setEventHandler(
            RevealEventTarget.shared, andSelector: #selector(RevealEventTarget.handleReveal(_:reply:)),
            forEventClass: AEEventClass(kAEMiscStandards), andEventID: AEEventID(kAEMakeObjectsVisible))
    }

    /// The engine is ready: handle what queued up.
    static func install(_ model: AppModel) {
        self.model = model
        #if DEBUG
        FileExplorerDebugSteps.register()
        #endif
        let queued = pending
        pending = []
        for q in queued { open(q.paths, reveal: q.reveal) }
    }

    /// Folders (or files) another app opened with File Studio.
    static func open(urls: [URL]) {
        open(urls.filter(\.isFileURL).map(\.path), reveal: false)
    }

    /// Opens or reveals paths through the core and brings the result forward.
    static func open(_ paths: [String], reveal: Bool) {
        guard !paths.isEmpty else { return }
        guard let model else { pending.append((paths, reveal)); return }
        print("SYSOPEN \(reveal ? "reveal" : "open") \(paths)")
        do {
            let out = try model.engine.openFromSystem(paths: paths, reveal: reveal)
            model.handle(out, in: nil)
            NSApp.activate()
            if let t = out.tab { bringForward(tab: t, tries: 20) }
        } catch let e as CoreError {
            model.statusMessage = e.message
            NSApp.activate()
            NSSound.beep()
        } catch {
            model.statusMessage = error.localizedDescription
        }
    }

    /// Makes the window showing `tab` key. Its controller appears once the
    /// core's change event reaches the app, so this retries briefly.
    private static func bringForward(tab: TabID, tries: Int) {
        if let wid = model?.window(of: tab)?.id,
           let w = NSApp.windows.first(where: { ($0.windowController as? CoreWindowIdentifying)?.coreWindowID == wid }) {
            if w.isMiniaturized { w.deminiaturize(nil) }
            w.makeKeyAndOrderFront(nil)
            return
        }
        guard tries > 0 else { return }
        let t = Timer(timeInterval: 0.05, repeats: false) { _ in
            MainActor.assumeIsolated { bringForward(tab: tab, tries: tries - 1) }
        }
        RunLoop.main.add(t, forMode: .common)
    }

    /// The file URLs in an Apple Event's direct object (one item or a list).
    nonisolated static func fileURLs(in event: NSAppleEventDescriptor) -> [URL] {
        guard let d = event.paramDescriptor(forKeyword: AEKeyword(keyDirectObject)) else { return [] }
        func url(_ x: NSAppleEventDescriptor) -> URL? {
            x.fileURLValue ?? x.coerce(toDescriptorType: DescType(typeFileURL))?.fileURLValue
        }
        if d.descriptorType == DescType(typeAEList) {
            return d.numberOfItems > 0 ? (1...d.numberOfItems).compactMap { d.atIndex($0).flatMap(url) } : []
        }
        return url(d).map { [$0] } ?? []
    }

    // MARK: Reveal in Finder

    /// File Studio's own Reveal in Finder: always Finder, even while File
    /// Studio is the default viewer.
    static func revealInFinder(_ paths: [String]) {
        let urls = paths.map { URL(fileURLWithPath: $0) }
        guard DefaultFileViewer.status().revealNeedsFinderEvent else {
            NSWorkspace.shared.activateFileViewerSelecting(urls)
            return
        }
        let finder = NSAppleEventDescriptor(bundleIdentifier: DefaultFileViewer.finderID)
        let ev = NSAppleEventDescriptor(eventClass: AEEventClass(kAEMiscStandards),
                                        eventID: AEEventID(kAEMakeObjectsVisible), targetDescriptor: finder,
                                        returnID: AEReturnID(kAutoGenerateReturnID),
                                        transactionID: AETransactionID(kAnyTransactionID))
        let list = NSAppleEventDescriptor.list()
        for (i, u) in urls.enumerated() { list.insert(NSAppleEventDescriptor(fileURL: u), at: i + 1) }
        ev.setParam(list, forKeyword: AEKeyword(keyDirectObject))
        do {
            _ = try ev.sendEvent(options: [.noReply, .canInteract], timeout: 5)
            NSRunningApplication.runningApplications(withBundleIdentifier: DefaultFileViewer.finderID).first?.activate()
        } catch {
            // No permission to script Finder: open the folder there instead.
            print("SYSOPEN finder reveal failed: \(error)")
            guard let finderURL = NSWorkspace.shared.urlForApplication(withBundleIdentifier: DefaultFileViewer.finderID)
            else { return }
            let folders = Array(Set(urls.map { $0.deletingLastPathComponent() }))
            NSWorkspace.shared.open(folders, withApplicationAt: finderURL, configuration: NSWorkspace.OpenConfiguration())
        }
    }
}

/// The Objective-C target `NSAppleEventManager` calls.
final class RevealEventTarget: NSObject, @unchecked Sendable {
    static let shared = RevealEventTarget()

    @objc func handleReveal(_ event: NSAppleEventDescriptor, reply: NSAppleEventDescriptor) {
        let paths = SystemOpen.fileURLs(in: event).map(\.path)
        MainActor.assumeIsolated { SystemOpen.open(paths, reveal: true) }
    }
}

extension EngineAPI {
    /// Engines without a real file system (the mock) can't open what other apps send.
    func openFromSystem(paths: [String], reveal: Bool) throws -> Outcome {
        throw CoreError(.notAvailable, "Opening folders from other apps needs the Rust core")
    }
}

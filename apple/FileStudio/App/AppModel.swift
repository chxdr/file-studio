// AppModel.swift — main-actor glue between the views and the core.
//
// It holds the engine, routes core events to per-window and per-tab models,
// runs commands and key presses through the core, and dispatches the
// `ShellRequest`s that come back to whoever registered for them. It decides
// nothing about files.
//
// ── Extension points (for the UI sessions; please don't edit this file) ──
//
// 1. Your own models. Put your model types in your own folder and hang them
//    off a window or tab with `attachment(_:make:)`:
//
//        let bar = windowModel.attachment(PathBarModel.self) { PathBarModel(window: windowModel) }
//
//    One instance per window/tab, created on first use, dropped with it.
//    `AppModel.attachment(_:make:)` does the same app-wide (the ops queue UI).
//
// 2. Events. `observe { event in … }` sees every `CoreEvent` on the main
//    actor after the built-in models are updated. Keep the token; observers
//    are removed when it is released or passed to `stopObserving`.
//
// 3. Shell requests. `registerShellHandler(for: "focusAddressBar") { req, ctx in … }`
//    (the kind names are `ShellRequest.kind`). Handlers run newest first;
//    return true when you handled it. `Platform/PlatformActions.swift`
//    registers the defaults (open, Quick Look, reveal, pasteboard…).
//
// 4. Running things. Views call `perform(_:via:)`, `handleKey(_:context:)`,
//    or the engine directly for queries (`model.engine.contextMenu(…)`), and
//    pass any `Outcome` they get to `handle(_:in:)`.

import AppKit
import Observation

// MARK: - Attachments

/// Per-object storage for models owned by the UI sessions (one per type).
@MainActor
public final class Attachments {
    private var store: [ObjectIdentifier: AnyObject] = [:]
    public init() {}
    public func get<T: AnyObject>(_ type: T.Type, make: () -> T) -> T {
        if let v = store[ObjectIdentifier(type)] as? T { return v }
        let v = make()
        store[ObjectIdentifier(type)] = v
        return v
    }
    public func existing<T: AnyObject>(_ type: T.Type) -> T? { store[ObjectIdentifier(type)] as? T }
}

// MARK: - Tab and window models

/// One tab as the views see it. Updated from core events; read rows through `api`.
@MainActor
@Observable
public final class TabModel {
    public let id: TabID
    @ObservationIgnored public let api: TabAPI
    /// The current listing (version, size, state). Changes on every `listingChanged`.
    public private(set) var listing: ListingInfo
    /// The kind of the last listing change.
    public private(set) var lastChange: ChangeKind = .sorted
    public private(set) var location: String
    public private(set) var selectionCount: UInt32 = 0
    public private(set) var cursor: UInt32?
    /// Bumped on every selection change, so row views can redraw.
    public private(set) var selectionVersion = 0
    public private(set) var canGoBack = false
    public private(set) var canGoForward = false
    @ObservationIgnored public let attachments = Attachments()

    init(api: TabAPI) {
        id = api.id
        self.api = api
        listing = api.listingInfo()
        location = api.location()
        canGoBack = api.canGoBack
        canGoForward = api.canGoForward
    }

    /// The folder's name for titles.
    public var title: String {
        let n = (location as NSString).lastPathComponent
        return n.isEmpty ? "/" : n
    }

    public func attachment<T: AnyObject>(_ type: T.Type, make: () -> T) -> T { attachments.get(type, make: make) }

    func listingChanged(_ kind: ChangeKind) {
        lastChange = kind
        listing = api.listingInfo()
        if location != listing.dir { location = listing.dir }
        canGoBack = api.canGoBack
        canGoForward = api.canGoForward
    }

    func selectionChanged(count: UInt32, cursor: UInt32?) {
        selectionCount = count
        self.cursor = cursor
        selectionVersion &+= 1
    }

    func locationMaybeChanged() {
        let l = api.location()
        if l != location { location = l }
        canGoBack = api.canGoBack
        canGoForward = api.canGoForward
    }
}

/// One window as the views see it (from `WorkspaceState`).
@MainActor
@Observable
public final class WindowModel {
    public let id: WindowID
    public private(set) var state: WindowState
    @ObservationIgnored public let attachments = Attachments()

    init(state: WindowState) { id = state.id; self.state = state }

    public var isDual: Bool { state.panes.count == 2 }
    public var activeTabID: TabID? { state.activeTabID }
    public func attachment<T: AnyObject>(_ type: T.Type, make: () -> T) -> T { attachments.get(type, make: make) }

    func update(_ s: WindowState) { if s != state { state = s } }
}

/// Where a shell request came from.
public struct ShellContext: Sendable {
    public var window: WindowID?
    public var tab: TabID?
}

/// Keeps an event observer alive.
@MainActor
public final class ObserverToken {
    fileprivate let id: Int
    fileprivate weak var model: AppModel?
    fileprivate init(id: Int, model: AppModel) { self.id = id; self.model = model }
    isolated deinit { model?.stopObserving(self) }
}

// MARK: - AppModel

@MainActor
@Observable
public final class AppModel {
    @ObservationIgnored public let engine: EngineAPI

    public private(set) var workspace = WorkspaceState()
    public private(set) var windows: [WindowID: WindowModel] = [:]
    public private(set) var tabs: [TabID: TabModel] = [:]
    /// This session's operations, in queue order, newest status of each.
    public private(set) var ops: [OpStatus] = []
    public private(set) var undoState = UndoState()
    public private(set) var volumes: [Volume] = []
    public private(set) var favourites: [Favourite] = []
    /// Every workspace, in switcher order; the active one has `active` set.
    public private(set) var workspaces: [WorkspaceSummary] = []
    public private(set) var settings = Settings()
    /// The last "Did you know?" hint (the toast session shows it).
    public private(set) var hint: Hint?
    /// A short status message from the last command ("Copied path").
    public var statusMessage: String? {
        didSet {
            statusStamp = Date()
            // Whatever shows it (a pane that isn't active may not), the message
            // goes after 6 s unless a newer one has replaced it.
            guard let m = statusMessage else { return }
            DispatchQueue.main.asyncAfter(deadline: .now() + 6) { [weak self] in
                MainActor.assumeIsolated { if self?.statusMessage == m { self?.statusMessage = nil } }
            }
        }
    }
    /// When `statusMessage` was last set.
    @ObservationIgnored private var statusStamp = Date.distantPast
    /// The unfinished key sequence (`g`).
    public private(set) var pendingKeys: String?
    /// Called when a window should open or close; set by the window layer.
    @ObservationIgnored var windowsDidChange: (() -> Void)?

    @ObservationIgnored private var observers: [(id: Int, fn: (CoreEvent) -> Void)] = []
    @ObservationIgnored private var nextObserver = 0
    @ObservationIgnored private var shellHandlers: [String: [(id: Int, fn: (ShellRequest, ShellContext) -> Bool)]] = [:]
    @ObservationIgnored private var appAttachments = Attachments()
    @ObservationIgnored private var pollTimer: Timer?

    public init(engine: EngineAPI) {
        self.engine = engine
        engine.setEventHandler { e in AppModel.onMain { AppModel.current?.route(e) } }
        AppModel.current = self
    }

    /// Runs `body` on the main thread, in order, from the main run loop (all
    /// common modes, so also during menu tracking and modal sheets). Not
    /// through the main dispatch queue and not as a Swift job: if view code
    /// in `body` makes AppKit throw an Objective-C exception that AppKit then
    /// swallows, unwinding through libdispatch's main-queue drain would wedge
    /// the queue (and every main-actor Task with it), and unwinding through a
    /// Swift job corrupts the runtime's executor tracking (later `@MainActor`
    /// checks crash). A run-loop block has neither.
    nonisolated static func onMain(_ body: @escaping @MainActor () -> Void) {
        let main = CFRunLoopGetMain()
        CFRunLoopPerformBlock(main, CFRunLoopMode.commonModes.rawValue) { MainActor.assumeIsolated { body() } }
        CFRunLoopWakeUp(main)
    }

    /// The app's model (one per process).
    public private(set) static var current: AppModel?

    /// Restores the session and loads sidebar data. Call once at launch.
    public func start() {
        settings = engine.settings()
        engine.volumesChanged()   // read volumes once, so crumbs show volume names
        workspace = engine.restoreSession()
        volumes = engine.volumes()
        favourites = engine.favourites()
        workspaces = (try? engine.workspaces()) ?? []
        ops = engine.opsList()
        undoState = engine.undoState()
        syncWorkspace()
    }

    public func attachment<T: AnyObject>(_ type: T.Type, make: () -> T) -> T { appAttachments.get(type, make: make) }

    /// Forgets finished operations (after `engine.clearFinishedOps()`); used by the ops popover.
    public func removeFinishedOps() { ops.removeAll { $0.state.isFinished } }

    // MARK: Lookups

    public func tabModel(_ id: TabID) -> TabModel? { tabs[id] }
    public func windowModel(_ id: WindowID) -> WindowModel? { windows[id] }
    /// The window that holds a tab.
    public func window(of tab: TabID) -> WindowModel? {
        windows.values.first { $0.state.panes.contains { $0.tabs.contains { $0.id == tab } } }
    }
    /// The focused tab of the key window (or the core's active tab).
    public var focusedTab: TabModel? {
        if let w = keyWindowID, let t = windows[w]?.activeTabID { return tabs[t] }
        return engine.activeTab().flatMap { tabs[$0.id] }
    }
    /// Set by the window layer when a window becomes key.
    public var keyWindowID: WindowID?

    // MARK: Events

    /// Sees every core event on the main actor, after the built-in models update.
    public func observe(_ fn: @escaping (CoreEvent) -> Void) -> ObserverToken {
        nextObserver += 1
        observers.append((nextObserver, fn))
        return ObserverToken(id: nextObserver, model: self)
    }

    public func stopObserving(_ token: ObserverToken) { observers.removeAll { $0.id == token.id } }

    func route(_ e: CoreEvent) {
        switch e {
        case .listingChanged(let t, _, let kind): tabs[t]?.listingChanged(kind)
        case .selectionChanged(let t, let n, let c):
            tabs[t]?.selectionChanged(count: n, cursor: c)
            clearStaleStatus()
        case .tabsChanged:
            clearStaleStatus()
            workspace = engine.workspaceState()
            syncWorkspace()
        case .opProgress(let list):
            for s in list {
                if let i = ops.firstIndex(where: { $0.id == s.id }) { ops[i] = s } else { ops.append(s) }
            }
        case .opConflict, .opFailed: break
        case .opFinished(let op, let report):
            if let i = ops.firstIndex(where: { $0.id == op }) { ops[i].state = report.state }
        case .undoChanged(let u): undoState = u
        case .volumesChanged(let v): volumes = v
        case .hint(let h): hint = h
        case .settingsChanged: settings = engine.settings()
        case .favouritesChanged: favourites = engine.favourites()
        case .workspacesChanged: workspaces = (try? engine.workspaces()) ?? workspaces
        }
        for o in observers { o.fn(e) }
    }

    /// A status message ("Nothing was moved") goes when the user moves on: a new
    /// selection or folder. One set a moment ago stays, because the command that
    /// set it may be what changed the selection. (The status line also fades
    /// every message after 5 s.)
    private func clearStaleStatus() {
        if statusMessage != nil, Date().timeIntervalSince(statusStamp) > 0.6 { statusMessage = nil }
    }

    /// Rebuilds the window and tab models from `workspace`, keeping existing ones.
    private func syncWorkspace() {
        var seenTabs = Set<TabID>()
        var seenWindows = Set<WindowID>()
        for w in workspace.windows {
            seenWindows.insert(w.id)
            if let m = windows[w.id] { m.update(w) } else { windows[w.id] = WindowModel(state: w) }
            for p in w.panes {
                for t in p.tabs {
                    seenTabs.insert(t.id)
                    if let m = tabs[t.id] { m.locationMaybeChanged() } else if let api = engine.tab(t.id) {
                        tabs[t.id] = TabModel(api: api)
                    }
                }
            }
        }
        for id in tabs.keys where !seenTabs.contains(id) { tabs[id] = nil }
        for id in windows.keys where !seenWindows.contains(id) { windows[id] = nil }
        windowsDidChange?()
    }

    // MARK: Running commands

    /// Runs a command and handles its outcome. Errors become a status message
    /// (and a beep for "not available").
    @discardableResult
    public func perform(_ inv: Invocation, via: Via, window: WindowID? = nil) -> Outcome? {
        Pasteboard.sync(engine)
        do {
            let out = try engine.perform(inv, via: via)
            handle(out, in: window ?? keyWindowID)
            return out
        } catch let e as CoreError {
            if e.kind == .notAvailable { NSSound.beep() }
            statusMessage = e.message
            return nil
        } catch {
            statusMessage = error.localizedDescription
            return nil
        }
    }

    /// Sends a key press to the core. Returns false when the core doesn't
    /// want it (let the focused control handle the event).
    public func handleKey(_ chord: KeyChord, context: InputContext, window: WindowID? = nil) -> Bool {
        Pasteboard.sync(engine)
        guard let out = engine.handleKey(chord, context: context) else { return false }
        handle(out, in: window ?? keyWindowID)
        return true
    }

    /// Applies an outcome: status message, pending keys, hint, shell request.
    public func handle(_ out: Outcome, in window: WindowID?) {
        if let m = out.message { statusMessage = m }
        if let h = out.hint { hint = h }
        pendingKeys = out.pendingKeys
        if pendingKeys != nil { startKeyPolling() }
        if let req = out.shell { dispatch(req, ShellContext(window: window, tab: out.tab)) }
    }

    private func startKeyPolling() {
        guard pollTimer == nil else { return }
        pollTimer = Timer.scheduledTimer(withTimeInterval: 0.1, repeats: true) { _ in
            MainActor.assumeIsolated {
                guard let self = AppModel.current else { return }
                let ctx = InputContext(focus: .list, tab: self.focusedTab?.id)
                if let out = self.engine.pollKeys(context: ctx) { self.handle(out, in: self.keyWindowID) }
                self.pendingKeys = self.engine.pendingKeys()
                if self.pendingKeys == nil { self.pollTimer?.invalidate(); self.pollTimer = nil }
            }
        }
    }

    // MARK: Shell requests

    /// Registers a handler for a `ShellRequest.kind`. Newest runs first;
    /// return true when handled. Keep the returned id to unregister.
    @discardableResult
    public func registerShellHandler(for kind: String, _ fn: @escaping (ShellRequest, ShellContext) -> Bool) -> Int {
        nextObserver += 1
        shellHandlers[kind, default: []].insert((nextObserver, fn), at: 0)
        return nextObserver
    }

    public func unregisterShellHandler(_ id: Int) {
        for k in shellHandlers.keys { shellHandlers[k]?.removeAll { $0.id == id } }
    }

    /// Runs the handlers for a request; logs it when nobody handles it.
    public func dispatch(_ req: ShellRequest, _ ctx: ShellContext) {
        for h in shellHandlers[req.kind] ?? [] where h.fn(req, ctx) { return }
        print("SHELL unhandled \(req)")
    }

    // MARK: OS hooks

    public func appWillQuit() { engine.appWillQuit() }
}

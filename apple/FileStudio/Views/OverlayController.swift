// OverlayController.swift — what makes the overlays appear.
//
// * Held modifiers: a local event monitor sees ⌘ ⇧ ⌥ ⌃ (or Fn alone) go down;
//   after the configured delay it asks the core (`heldShortcuts`) what the
//   chord would do and shows the answer. It knows no shortcuts itself.
// * Hints: `AppModel.hint` (set from any `Outcome` or `.hint` event) is shown
//   for the configured time.
// * Toasts: new `statusMessage`s (undo refusals, errors) become toasts.
// * The Edit menu's Undo/Redo titles follow `AppModel.undoState`.
//
// Hosting: one transparent, click-through view per core window (OverlayHost).

import AppKit
import SwiftUI

@MainActor
final class OverlayController {
    static let shared = OverlayController()
    private(set) var model: AppModel!
    let state = OverlayState()
    private var monitor: Any?
    private var heldTask: Task<Void, Never>?
    private var hintTask: Task<Void, Never>?
    private var hosts: [ObjectIdentifier: OverlayHost] = [:]
    private var notes: [NSObjectProtocol] = []
    private var lastCoreKey: ObjectIdentifier?

    /// Call once, after the model started and the windows exist.
    func install(_ model: AppModel) {
        guard self.model == nil else { return }
        self.model = model
        monitor = NSEvent.addLocalMonitorForEvents(matching: [.flagsChanged, .keyDown]) { [weak self] e in
            MainActor.assumeIsolated {
                if e.type == .flagsChanged { self?.modifiersChanged(e.modifierFlags) } else if self?.state.held == nil {
                    // A chord pressed before the overlay appeared: the user knows it.
                    self?.heldTask?.cancel()
                }
            }
            return e
        }
        let nc = NotificationCenter.default
        notes.append(nc.addObserver(forName: NSApplication.didResignActiveNotification, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { self?.modifiersChanged([]) }
        })
        for name in [NSWindow.didBecomeKeyNotification, NSWindow.didResignKeyNotification,
                     NSWindow.didBecomeMainNotification] {
            notes.append(nc.addObserver(forName: name, object: nil, queue: .main) { [weak self] _ in
                MainActor.assumeIsolated { self?.windowsChanged(NSApp.keyWindow) }
            })
        }
        track { $0.hint } then: { [weak self] in self?.hintChanged() }
        track { $0.statusMessage } then: { [weak self] in self?.statusChanged() }
        track { $0.undoState } then: { [weak self] in self?.updateUndoTitles() }
        updateUndoTitles()
        windowsChanged(nil)
    }

    /// Re-runs `then` whenever the observed value changes.
    private func track<T>(_ read: @escaping @MainActor (AppModel) -> T, then: @escaping @MainActor () -> Void) {
        let m = model!
        withObservationTracking { _ = read(m) } onChange: { [weak self] in
            Task { @MainActor in
                then()
                self?.track(read, then: then)
            }
        }
    }

    // MARK: Hosts

    private func windowsChanged(_ w: NSWindow?) {
        if let w, w.isKeyWindow, w.windowController is CoreWindowIdentifying { lastCoreKey = ObjectIdentifier(w) }
        let cores = NSApp.windows.filter { $0.windowController is CoreWindowIdentifying && $0.isVisible }
        if lastCoreKey == nil || !cores.contains(where: { ObjectIdentifier($0) == lastCoreKey }) {
            lastCoreKey = (cores.first { $0.isKeyWindow } ?? cores.first).map { ObjectIdentifier($0) }
        }
        for win in NSApp.windows where win.windowController is CoreWindowIdentifying && win.isVisible {
            let id = ObjectIdentifier(win)
            if hosts[id]?.superview == nil || hosts[id]?.window !== win {
                let h = OverlayHost(window: win, model: model, state: state)
                hosts[id] = h
            }
        }
        hosts = hosts.filter { $0.value.window != nil }
        for (id, h) in hosts { h.isHidden = !(id == lastCoreKey) }
    }

    // MARK: Held modifiers

    func modifiersChanged(_ flags: NSEvent.ModifierFlags, in window: NSWindow? = NSApp.keyWindow) {
        let f = flags.intersection(.deviceIndependentFlagsMask)
        let mods = Mods(f)
        let fnOnly = mods.isEmpty && f.contains(.function)
        let o = model.settings.overlay
        heldTask?.cancel()
        let w = window
        guard !mods.isEmpty || fnOnly, o.enabled, !(fnOnly && !o.listPlainKeys),
              w?.windowController is CoreWindowIdentifying, w?.attachedSheet == nil else {
            hideHeld()
            return
        }
        // Shift while typing in a text field is just capitals.
        if mods.shift && !mods.cmd && !mods.alt && !mods.ctrl, w?.firstResponder is NSText { hideHeld(); return }
        let ctx = KeyRouter.context(in: w).0
        let lookup = { [engine = model.engine] in engine.heldShortcuts(mods, context: ctx) }
        let appearance = AppearanceModel.of(model)
        if state.held != nil {
            withAnimation(appearance.animation(.snappy(duration: 0.25))) { state.held = lookup() }
            return
        }
        let delay = Int(o.delayMs)
        heldTask = Task { @MainActor [weak self] in
            try? await Task.sleep(for: .milliseconds(delay))
            guard !Task.isCancelled, let self, let held = lookup() else { return }
            if self.state.hint != nil { self.dismissHint() }
            withAnimation(appearance.animation(.spring(duration: 0.5, bounce: 0.22))) { self.state.held = held }
        }
    }

    private func hideHeld() {
        guard state.held != nil else { return }
        withAnimation(AppearanceModel.of(model).animation(.smooth(duration: 0.25))) { state.held = nil }
    }

    // MARK: Hints

    private func hintChanged() {
        guard let h = model.hint, model.settings.hints.enabled, state.held == nil else { return }
        hintTask?.cancel()
        withAnimation(AppearanceModel.of(model).animation(.spring(duration: 0.45, bounce: 0.28))) { state.hint = h }
        let secs = Double(model.settings.hints.seconds)
        hintTask = Task { @MainActor [weak self] in
            try? await Task.sleep(for: .seconds(secs))
            guard !Task.isCancelled else { return }
            self?.dismissHint()
        }
    }

    func dismissHint() {
        hintTask?.cancel()
        withAnimation(AppearanceModel.of(model).animation(.smooth(duration: 0.3))) { state.hint = nil }
    }

    /// Shows a hint right now (debug automation and tests).
    func showHint(_ h: Hint) {
        withAnimation(AppearanceModel.of(model).animation(.spring(duration: 0.45, bounce: 0.28))) { state.hint = h }
    }

    // MARK: Messages and menus

    private func statusChanged() {
        guard let m = model.statusMessage, !m.isEmpty else { return }
        // The core may send the same words as a styled toast (a warning);
        // don't replace that with a plain one.
        let center = ToastCenter.of(model)
        if center.toasts.contains(where: { $0.text == m }) { return }
        center.show(m, seconds: 3)
    }

    /// "Undo Move of 12 Items": the core words it.
    private func updateUndoTitles() {
        func walk(_ menu: NSMenu) {
            for it in menu.items {
                switch it.representedObject as? String {
                case "edit.undo": it.title = model.undoState.undo ?? "Undo"
                case "edit.redo": it.title = model.undoState.redo ?? "Redo"
                default: break
                }
                if let s = it.submenu { walk(s) }
            }
        }
        if let m = NSApp.mainMenu { walk(m) }
    }
}

/// A transparent view over a window's content that passes clicks through
/// except where a toast or hint is drawn.
@MainActor
final class OverlayHost: NSHostingView<OverlayRoot> {
    init(window: NSWindow, model: AppModel, state: OverlayState) {
        let root = OverlayRoot(model: model, state: state, toasts: ToastCenter.of(model), appearance: AppearanceModel.of(model))
        super.init(rootView: root)
        translatesAutoresizingMaskIntoConstraints = true
        autoresizingMask = [.width, .height]
        if let content = window.contentView, let frame = content.superview {
            self.frame = content.frame
            frame.addSubview(self, positioned: .above, relativeTo: nil)
        }
    }
    required init(rootView: OverlayRoot) { fatalError("use init(window:model:state:)") }
    @available(*, unavailable) required init?(coder: NSCoder) { fatalError() }

    override func hitTest(_ point: NSPoint) -> NSView? {
        let v = super.hitTest(point)
        return v === self ? nil : v
    }
}

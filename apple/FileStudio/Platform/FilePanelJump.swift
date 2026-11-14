// FilePanelJump.swift — send another app's Open or Save panel to the folder
// in File Studio's active pane (Settings › General › Open and Save Panels).
//
// The core decides: whether it's on (`integration.panel_jump`), the
// shortcut (`panel.jump_here` in the keymap; `global_hotkey_chord` says if
// it can be a system hotkey), the folder (the command's `jumpFilePanel`
// request), whether a window is a file panel (`classify_file_panel`), and
// where the floating button goes (`panel_button_origin`). This file makes
// the macOS calls: a Carbon hotkey, Accessibility reads, and the keystrokes
// a person would type (⌘⇧G, the path, Return).

import AppKit
import ApplicationServices
import Carbon.HIToolbox

@MainActor
final class FilePanelJump {
    static let shared = FilePanelJump()

    private weak var model: AppModel?
    private var settingsToken: ObserverToken?
    private var activation: NSObjectProtocol?
    /// The last app other than File Studio that was in front: the palette
    /// and menus run the command from inside File Studio.
    private var lastOtherApp: NSRunningApplication?
    private var hotKey: EventHotKeyRef?
    private var hotKeyHandler: EventHandlerRef?
    private var hotKeyText: String?
    private var pollTimer: Timer?
    private var button: PanelJumpButton?
    private var busy = false
    /// Why the shortcut isn't registered, for Settings.
    private(set) var hotKeyProblem: String?

    // MARK: Setup

    func install(_ model: AppModel) {
        self.model = model
        model.registerShellHandler(for: "jumpFilePanel") { [weak self] req, _ in
            if case .jumpFilePanel(let dir) = req { self?.jump(to: dir) }
            return true
        }
        activation = NSWorkspace.shared.notificationCenter.addObserver(
            forName: NSWorkspace.didActivateApplicationNotification, object: nil, queue: .main) { [weak self] n in
            let app = n.userInfo?[NSWorkspace.applicationUserInfoKey] as? NSRunningApplication
            MainActor.assumeIsolated {
                if let app, app.processIdentifier != getpid() { self?.lastOtherApp = app }
            }
        }
        settingsToken = model.observe { [weak self] e in
            if case .settingsChanged = e { self?.apply() }
        }
        apply()
    }

    var isOn: Bool { model.map { Core.bool($0.engine, "integration.panel_jump") ?? false } ?? false }
    private var buttonOn: Bool { model.map { Core.bool($0.engine, "integration.panel_jump_button") ?? true } ?? true }

    /// Registers or drops the hotkey and the button to match the settings.
    func apply() {
        guard let model else { return }
        let keys = isOn ? model.engine.commandKeys("panel.jump_here").first : nil
        if keys != hotKeyText {
            unregisterHotKey()
            hotKeyText = keys
            if let keys { registerHotKey(keys) } else { hotKeyProblem = nil }
        }
        if isOn && buttonOn { startPolling() } else { stopPolling() }
    }

    // MARK: Accessibility permission

    static var isTrusted: Bool { AXIsProcessTrusted() }

    /// Shows macOS's own "allow Accessibility" prompt (once per app; after
    /// that it does nothing, and Settings offers the Privacy pane instead).
    static func requestTrust() {
        let opts = ["AXTrustedCheckOptionPrompt": true] as CFDictionary // kAXTrustedCheckOptionPrompt
        _ = AXIsProcessTrustedWithOptions(opts)
    }

    static func openPrivacySettings() {
        if let u = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility") {
            NSWorkspace.shared.open(u)
        }
    }

    // MARK: Hotkey

    private func registerHotKey(_ keys: String) {
        guard let chord = globalHotkeyChord(keys: keys), let code = KeyCodes.code(for: chord.key) else {
            hotKeyProblem = "\(keys) can't be a system-wide shortcut: give it ⌘, ⌃ or ⌥ and a single key."
            return
        }
        var mods: UInt32 = 0
        if chord.mods.cmd { mods |= UInt32(cmdKey) }
        if chord.mods.alt { mods |= UInt32(optionKey) }
        if chord.mods.ctrl { mods |= UInt32(controlKey) }
        if chord.mods.shift { mods |= UInt32(shiftKey) }
        if hotKeyHandler == nil {
            var spec = EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
            InstallEventHandler(GetApplicationEventTarget(), { _, _, _ in
                // Carbon delivers hotkeys on the main thread.
                MainActor.assumeIsolated { FilePanelJump.shared.hotKeyPressed() }
                return noErr
            }, 1, &spec, nil, &hotKeyHandler)
        }
        let id = EventHotKeyID(signature: OSType(0x4653_504A), id: 1) // 'FSPJ'
        let err = RegisterEventHotKey(code, mods, id, GetApplicationEventTarget(), 0, &hotKey)
        hotKeyProblem = err == noErr ? nil : "\(keys) is already taken by another app."
        print("PANELJUMP hotkey \(keys) code=\(code) mods=\(mods) err=\(err)")
    }

    private func unregisterHotKey() {
        if let hotKey { UnregisterEventHotKey(hotKey) }
        hotKey = nil
    }

    private func hotKeyPressed() { runCommand() }

    /// Runs `panel.jump_here` through the core; its `jumpFilePanel` request
    /// comes back to `jump(to:)`.
    /// Debug automation: drive `bundleID`'s panel as if it had been in front.
    func runCommand(targeting bundleID: String) {
        lastOtherApp = NSRunningApplication.runningApplications(withBundleIdentifier: bundleID).first
        runCommand()
    }

    func runCommand() {
        guard let model else { return }
        model.perform(Invocation(id: "panel.jump_here", tab: nil), via: .keyboard)
    }

    // MARK: The jump

    /// The app whose panel to drive: the one in front, or the last one in
    /// front when File Studio is (the palette, the menu).
    private func targetApp() -> NSRunningApplication? {
        let front = NSWorkspace.shared.frontmostApplication
        if let front, front.processIdentifier != getpid() { return front }
        return lastOtherApp.flatMap { $0.isTerminated ? nil : $0 }
    }

    func jump(to dir: String, in override: NSRunningApplication? = nil) {
        guard !busy else { return }
        guard Self.isTrusted else {
            fail("Allow File Studio in Privacy & Security › Accessibility to send Open and Save panels to a folder.")
            Self.requestTrust()
            return
        }
        guard let app = override ?? targetApp() else { return fail("Open an Open or Save panel in another app first.") }
        let axApp = AXUIElementCreateApplication(app.processIdentifier)
        AXUIElementSetMessagingTimeout(axApp, 0.5)
        guard let panel = PanelFinder.find(in: axApp) else {
            return fail("No Open or Save panel in \(app.localizedName ?? "the app in front").")
        }
        busy = true
        print("PANELJUMP \(app.bundleIdentifier ?? "?") \(panel.kind) -> \(dir)")
        let wasActive = app.isActive
        if !wasActive { app.activate() }
        AXUIElementPerformAction(panel.window, kAXRaiseAction as CFString)
        let before = AX.element(axApp, kAXFocusedUIElementAttribute)
        let text = panelJumpText(folder: dir)
        let target = KeyTarget(app: app, axApp: axApp, panel: panel)
        // Wait for the hotkey's modifiers to come up, so they don't mix into ⌘⇧G.
        waitForModifiersUp(tries: 40) { [weak self] in
            guard let self else { return }
            // Every keystroke goes to whatever is in front: check first.
            if let why = target.refusal(field: nil, expect: nil) { return self.abort(why) }
            Keys.post(UInt16(kVK_ANSI_G), [.maskCommand, .maskShift])
            self.waitForGoToField(axApp, before: before, tries: 30) { field in
                guard let field else {
                    return self.abort("\(app.localizedName ?? "The app")'s panel didn't open its Go to Folder field.")
                }
                self.fill(field, with: text, target: target) {
                    // Read the value back right now, and only then press Return.
                    if let why = target.refusal(field: field, expect: text) { return self.abort(why) }
                    Keys.post(UInt16(kVK_Return), [])
                    self.busy = false
                    if let b = self.button { b.flash() }
                }
            }
        }
    }

    private func abort(_ why: String) {
        busy = false
        fail(why)
    }

    /// Puts `text` in the Go to Folder field: through Accessibility when the
    /// field takes it, else by typing it (each keystroke checked first).
    private func fill(_ field: AXUIElement, with text: String, target: KeyTarget,
                      then done: @escaping @MainActor () -> Void) {
        let ok = AXUIElementSetAttributeValue(field, kAXValueAttribute as CFString, text as CFTypeRef) == .success
            && (AX.string(field, kAXValueAttribute) == text)
        if ok {
            print("PANELJUMP set by AX")
            after(0.12, done)
            return
        }
        print("PANELJUMP typing")
        if let why = target.refusal(field: field, expect: nil) { return abort(why) }
        Keys.post(UInt16(kVK_ANSI_A), [.maskCommand])
        after(0.05) { [weak self] in
            if let why = target.refusal(field: field, expect: nil) { self?.abort(why); return }
            Keys.type(text)
            after(0.2, done)
        }
    }

    private func waitForModifiersUp(tries: Int, _ then: @escaping @MainActor () -> Void) {
        let held = CGEventSource.flagsState(.combinedSessionState)
            .intersection([.maskCommand, .maskControl, .maskAlternate, .maskShift])
        if held.isEmpty || tries == 0 { return after(0.05, then) }
        after(0.03) { [weak self] in self?.waitForModifiersUp(tries: tries - 1, then) }
    }

    /// Waits for a text field other than `before` to take focus (the Go to
    /// Folder field; a Save panel's name field had it before).
    private func waitForGoToField(_ app: AXUIElement, before: AXUIElement?, tries: Int,
                                  _ then: @escaping @MainActor (AXUIElement?) -> Void) {
        if let f = AX.element(app, kAXFocusedUIElementAttribute),
           ["AXTextField", "AXComboBox", "AXSearchField"].contains(AX.string(f, kAXRoleAttribute) ?? ""),
           before.map({ !CFEqual($0, f) }) ?? true {
            return then(f)
        }
        guard tries > 0 else { return then(nil) }
        after(0.05) { [weak self] in self?.waitForGoToField(app, before: before, tries: tries - 1, then) }
    }

    private func fail(_ text: String) {
        print("PANELJUMP fail: \(text)")
        NSSound.beep()
        model?.statusMessage = text
        button?.show(message: text)
    }

    // MARK: Floating button

    private func startPolling() {
        guard pollTimer == nil else { return }
        let t = Timer(timeInterval: 0.5, repeats: true) { _ in
            MainActor.assumeIsolated { FilePanelJump.shared.poll() }
        }
        RunLoop.main.add(t, forMode: .common)
        pollTimer = t
    }

    private func stopPolling() {
        pollTimer?.invalidate()
        pollTimer = nil
        button?.hide()
    }

    private func poll() {
        guard !busy else { return }
        guard Self.isTrusted, let front = NSWorkspace.shared.frontmostApplication,
              front.processIdentifier != getpid(), let model else { button?.hide(); return }
        let axApp = AXUIElementCreateApplication(front.processIdentifier)
        AXUIElementSetMessagingTimeout(axApp, 0.25)
        guard let panel = PanelFinder.find(in: axApp, deep: false) else { button?.hide(); return }
        let dir = model.focusedTab?.location ?? model.engine.home()
        if button == nil { button = PanelJumpButton { FilePanelJump.shared.runCommand() } }
        button?.show(title: panelButtonTitle(folder: dir), near: panel.frame)
    }
}

/// The panel being driven, and the check that runs before each keystroke.
@MainActor
struct KeyTarget {
    let app: NSRunningApplication
    let axApp: AXUIElement
    let panel: FoundPanel

    /// Why the next keystroke must not be sent (the core decides), or `nil`.
    /// `field`: the Go to Folder field once it is open; `expect`: the value
    /// it must hold right now (before Return).
    func refusal(field: AXUIElement?, expect: String?) -> String? {
        let front = NSWorkspace.shared.frontmostApplication?.processIdentifier == app.processIdentifier
        let open = AX.string(panel.element, kAXRoleAttribute) != nil
        let focused = field.map { f in
            AX.element(axApp, kAXFocusedUIElementAttribute).map { CFEqual($0, f) } ?? false
        } ?? true
        let value: Bool? = expect.map { e in field.flatMap { AX.string($0, kAXValueAttribute) } == e }
        return panelKeyRefusal(appInFront: front, fieldFocused: focused, panelOpen: open, valueMatches: value)
    }
}

// MARK: - Finding the panel

/// A file panel found through Accessibility.
@MainActor
struct FoundPanel {
    /// The window to raise (the panel itself, or the window a sheet hangs off).
    let window: AXUIElement
    /// The panel itself (the window, or the sheet).
    let element: AXUIElement
    let kind: FfiPanelKind
    /// Its frame in Accessibility (top-left-origin) coordinates.
    let frame: CGRect
}

@MainActor
enum PanelFinder {
    /// The Open or Save panel of an app: its focused window or a sheet on it,
    /// then its other windows. `deep: false` skips windows that aren't
    /// dialogs or sheets (the button polls twice a second).
    static func find(in app: AXUIElement, deep: Bool = true) -> FoundPanel? {
        var windows: [AXUIElement] = []
        if let w = AX.element(app, kAXFocusedWindowAttribute) { windows.append(w) }
        for w in AX.elements(app, kAXWindowsAttribute).prefix(8) where !windows.contains(where: { CFEqual($0, w) }) {
            windows.append(w)
        }
        for w in windows {
            let sheets = AX.elements(w, kAXChildrenAttribute).filter { AX.string($0, kAXRoleAttribute) == "AXSheet" }
            for c in sheets + [w] {
                if !deep, CFEqual(c, w), !isDialogish(c) { continue }
                if let kind = classifyFilePanel(probe: probe(c)) {
                    return FoundPanel(window: w, element: c, kind: kind, frame: AX.frame(c) ?? .zero)
                }
            }
        }
        return nil
    }

    private static func isDialogish(_ e: AXUIElement) -> Bool {
        let sub = AX.string(e, kAXSubroleAttribute) ?? ""
        let id = AX.string(e, kAXIdentifierAttribute) ?? ""
        return sub == "AXDialog" || sub == "AXSystemDialog" || id.contains("panel")
    }

    /// What the core needs to classify `e`: its roles and identifier, and the
    /// buttons and roles a few levels down (bounded, so a web page's
    /// thousands of elements aren't walked).
    static func probe(_ e: AXUIElement) -> FfiPanelProbe {
        var buttons: [String] = [], roles: Set<String> = []
        let id = AX.string(e, kAXIdentifierAttribute) ?? ""
        if !id.contains("panel") {
            var queue: [(AXUIElement, Int)] = AX.elements(e, kAXChildrenAttribute).map { ($0, 1) }
            var seen = 0
            while !queue.isEmpty, seen < 400 {
                let (x, depth) = queue.removeFirst()
                seen += 1
                let role = AX.string(x, kAXRoleAttribute) ?? ""
                roles.insert(role)
                if role == "AXButton", let t = AX.string(x, kAXTitleAttribute), !t.isEmpty { buttons.append(t) }
                // Don't descend into lists of files: their rows say nothing more.
                if depth < 7, !["AXOutline", "AXBrowser", "AXTable", "AXWebArea"].contains(role) {
                    queue += AX.elements(x, kAXChildrenAttribute).map { ($0, depth + 1) }
                }
            }
        }
        return FfiPanelProbe(role: AX.string(e, kAXRoleAttribute) ?? "", subrole: AX.string(e, kAXSubroleAttribute) ?? "",
                             identifier: id, buttons: buttons, innerRoles: Array(roles).sorted())
    }
}

// MARK: - Accessibility and keyboard helpers

@MainActor
enum AX {
    static func value(_ e: AXUIElement, _ attr: String) -> CFTypeRef? {
        var v: CFTypeRef?
        return AXUIElementCopyAttributeValue(e, attr as CFString, &v) == .success ? v : nil
    }

    static func string(_ e: AXUIElement, _ attr: String) -> String? { value(e, attr) as? String }

    static func element(_ e: AXUIElement, _ attr: String) -> AXUIElement? {
        guard let v = value(e, attr), CFGetTypeID(v) == AXUIElementGetTypeID() else { return nil }
        return (v as! AXUIElement) // swiftlint:disable:this force_cast
    }

    static func elements(_ e: AXUIElement, _ attr: String) -> [AXUIElement] {
        (value(e, attr) as? [AnyObject] ?? []).compactMap {
            CFGetTypeID($0) == AXUIElementGetTypeID() ? ($0 as! AXUIElement) : nil // swiftlint:disable:this force_cast
        }
    }

    static func frame(_ e: AXUIElement) -> CGRect? {
        guard let p = value(e, kAXPositionAttribute), let s = value(e, kAXSizeAttribute),
              CFGetTypeID(p) == AXValueGetTypeID(), CFGetTypeID(s) == AXValueGetTypeID() else { return nil }
        var pt = CGPoint.zero, sz = CGSize.zero
        AXValueGetValue(p as! AXValue, .cgPoint, &pt) // swiftlint:disable:this force_cast
        AXValueGetValue(s as! AXValue, .cgSize, &sz) // swiftlint:disable:this force_cast
        return CGRect(origin: pt, size: sz)
    }
}

/// Synthetic key presses to the app in front (needs Accessibility).
@MainActor
enum Keys {
    static func post(_ code: UInt16, _ flags: CGEventFlags) {
        let src = CGEventSource(stateID: .hidSystemState)
        for down in [true, false] {
            let e = CGEvent(keyboardEventSource: src, virtualKey: code, keyDown: down)
            e?.flags = flags
            e?.post(tap: .cghidEventTap)
        }
    }

    /// Types text as Unicode, a few characters per event.
    static func type(_ text: String) {
        let src = CGEventSource(stateID: .hidSystemState)
        var chunk: [UniChar] = []
        func flush() {
            guard !chunk.isEmpty else { return }
            for down in [true, false] {
                let e = CGEvent(keyboardEventSource: src, virtualKey: 0, keyDown: down)
                e?.flags = []
                e?.keyboardSetUnicodeString(stringLength: chunk.count, unicodeString: chunk)
                e?.post(tap: .cghidEventTap)
            }
            chunk.removeAll()
        }
        for u in text.utf16 {
            chunk.append(u)
            if chunk.count == 16 { flush() }
        }
        flush()
    }
}

/// Virtual key codes for the hotkey, using the current keyboard layout for
/// characters (so a hotkey on "G" means the key that types g).
@MainActor
enum KeyCodes {
    static func code(for key: FfiKey) -> UInt32? {
        switch key {
        case .char(let s): return s.first.flatMap(charCode)
        case .left: return UInt32(kVK_LeftArrow)
        case .right: return UInt32(kVK_RightArrow)
        case .up: return UInt32(kVK_UpArrow)
        case .down: return UInt32(kVK_DownArrow)
        case .space: return UInt32(kVK_Space)
        case .return: return UInt32(kVK_Return)
        case .enter: return UInt32(kVK_ANSI_KeypadEnter)
        case .escape: return UInt32(kVK_Escape)
        case .tab: return UInt32(kVK_Tab)
        case .delete: return UInt32(kVK_Delete)
        case .forwardDelete: return UInt32(kVK_ForwardDelete)
        case .home: return UInt32(kVK_Home)
        case .end: return UInt32(kVK_End)
        case .pageUp: return UInt32(kVK_PageUp)
        case .pageDown: return UInt32(kVK_PageDown)
        case .f(let n):
            let f = [kVK_F1, kVK_F2, kVK_F3, kVK_F4, kVK_F5, kVK_F6, kVK_F7, kVK_F8, kVK_F9, kVK_F10,
                     kVK_F11, kVK_F12, kVK_F13, kVK_F14, kVK_F15, kVK_F16, kVK_F17, kVK_F18, kVK_F19, kVK_F20]
            return (1...20).contains(Int(n)) ? UInt32(f[Int(n) - 1]) : nil
        }
    }

    private static func charCode(_ c: Character) -> UInt32? {
        let want = String(c).lowercased()
        if let src = TISCopyCurrentKeyboardLayoutInputSource()?.takeRetainedValue(),
           let raw = TISGetInputSourceProperty(src, kTISPropertyUnicodeKeyLayoutData) {
            let data = Unmanaged<CFData>.fromOpaque(raw).takeUnretainedValue() as Data
            let found: UInt32? = data.withUnsafeBytes { buf in
                guard let layout = buf.baseAddress?.assumingMemoryBound(to: UCKeyboardLayout.self) else { return nil }
                for code in 0..<UInt16(128) {
                    var dead: UInt32 = 0, len = 0
                    var chars = [UniChar](repeating: 0, count: 4)
                    let err = UCKeyTranslate(layout, code, UInt16(kUCKeyActionDisplay), 0, UInt32(LMGetKbdType()),
                                             OptionBits(kUCKeyTranslateNoDeadKeysBit), &dead, 4, &len, &chars)
                    if err == noErr, len > 0, String(utf16CodeUnits: chars, count: len).lowercased() == want {
                        return UInt32(code)
                    }
                }
                return nil
            }
            if let found { return found }
        }
        return nil
    }
}

@MainActor
private func after(_ s: TimeInterval, _ body: @escaping @MainActor () -> Void) {
    let t = Timer(timeInterval: s, repeats: false) { _ in MainActor.assumeIsolated { body() } }
    RunLoop.main.add(t, forMode: .common)
}

// MARK: - The floating button

/// A small borderless button that floats by another app's file panel. It
/// never activates File Studio, so the panel's app stays in front.
@MainActor
final class PanelJumpButton {
    private let panel: NSPanel
    private let button: NSButton
    private let action: @MainActor () -> Void
    private var messageUntil = Date.distantPast

    init(action: @escaping @MainActor () -> Void) {
        self.action = action
        panel = NSPanel(contentRect: NSRect(x: 0, y: 0, width: 180, height: 28),
                        styleMask: [.nonactivatingPanel, .borderless], backing: .buffered, defer: true)
        panel.level = .statusBar
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = true
        panel.hidesOnDeactivate = false
        panel.becomesKeyOnlyIfNeeded = true
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .transient, .ignoresCycle]
        button = NSButton(title: "", target: nil, action: nil)
        button.bezelStyle = .glass
        button.controlSize = .regular
        button.image = NSImage(systemSymbolName: "arrow.right.circle.fill", accessibilityDescription: nil)
        button.imagePosition = .imageLeading
        button.setAccessibilityIdentifier("panel-jump-button")
        panel.contentView = button
        button.target = self
        button.action = #selector(clicked)
    }

    @objc private func clicked() { action() }

    /// Shows the button by a panel whose frame is in Accessibility coordinates.
    func show(title: String, near ax: CGRect) {
        if Date() < messageUntil { return }
        if button.title != title { button.title = title }
        button.toolTip = "Send this panel to the folder in File Studio's active pane"
        let size = button.fittingSize
        let w = max(120, size.width + 16), h = max(26, size.height + 4)
        let primaryH = NSScreen.screens.first?.frame.height ?? 0
        let center = CGPoint(x: ax.midX, y: primaryH - ax.midY)
        let screen = NSScreen.screens.first { $0.frame.contains(center) } ?? NSScreen.main
        let vf = screen?.visibleFrame ?? .zero
        let screenAX = FfiScreenRect(x: vf.minX, y: primaryH - vf.maxY, w: vf.width, h: vf.height)
        let o = panelButtonOrigin(panel: FfiScreenRect(x: ax.minX, y: ax.minY, w: ax.width, h: ax.height),
                                  buttonW: w, buttonH: h, screen: screenAX)
        panel.setFrame(NSRect(x: o.x, y: primaryH - o.y - h, width: w, height: h), display: true)
        if !panel.isVisible { panel.orderFrontRegardless() }
    }

    /// Replaces the title with a short message for a few seconds.
    func show(message: String) {
        guard panel.isVisible else { return }
        button.title = message.count > 60 ? String(message.prefix(57)) + "…" : message
        messageUntil = Date().addingTimeInterval(3)
    }

    func flash() {
        button.highlight(true)
        after(0.15) { [weak self] in self?.button.highlight(false) }
    }

    func hide() { if panel.isVisible { panel.orderOut(nil) } }
}

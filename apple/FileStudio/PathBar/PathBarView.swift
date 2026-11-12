// PathBarView.swift — the address bar (F-009, F-018, HANDS-ON §8 and §10).
//
// Always visible at the top of each pane. Three states:
//   REST        crumbs (CrumbStripView); flat, a capsule outline on hover.
//   EDIT        a text field with the folder's path + "/", all selected.
//   COMPLETING  the same field plus a suggestion list under the bar.
// Keys reach the core first (KeyRouter, focus `.addressBar`); the core's
// AddressBarEditing bindings come back as `ShellRequest.textField(command:)`
// and are carried out here. Completion, Tab cycling, ⌥⌫, parsing, the paste
// cleanup, Create Folder and drops are all core calls; this file draws them
// and moves text around.

import AppKit
import os

/// The name the window layer uses (kept from the M2 placeholder).
typealias PathBarPlaceholderView = PathBarView

/// A text field that tells KeyRouter which focus it is.
@MainActor
class CoreTextField: NSTextField, CoreFocusReporting {
    var coreFocus: Focus = .addressBar
    var coreTab: TabID?
    var onBecomeFirstResponder: (() -> Void)?

    override func becomeFirstResponder() -> Bool {
        let ok = super.becomeFirstResponder()
        if ok { onBecomeFirstResponder?() }
        return ok
    }
}

/// The address field: a combo box for VoiceOver, with its list as the popup.
@MainActor
final class AddressField: CoreTextField {
    override func accessibilityRole() -> NSAccessibility.Role? { .comboBox }

    /// ⌘Z / ⇧⌘Z while editing: the field editor keeps its own undo stack (the
    /// paste cleanup is undoable there), which the window's `undo:` doesn't see.
    private var editorUndo: UndoManager? { (currentEditor() as? NSTextView)?.undoManager }
    @objc func undo(_ sender: Any?) {
        if editorUndo?.canUndo == true { editorUndo?.undo() } else { _ = nextResponder?.tryToPerform(#selector(undo(_:)), with: sender) }
    }
    @objc func redo(_ sender: Any?) {
        if editorUndo?.canRedo == true { editorUndo?.redo() } else { _ = nextResponder?.tryToPerform(#selector(redo(_:)), with: sender) }
    }
    /// ⌘Z / ⇧⌘Z reach the field before the Edit menu (the core declines them here).
    override func performKeyEquivalent(with event: NSEvent) -> Bool {
        guard currentEditor() != nil, event.type == .keyDown,
              event.charactersIgnoringModifiers?.lowercased() == "z" else { return super.performKeyEquivalent(with: event) }
        let mods = event.modifierFlags.intersection([.command, .shift, .option, .control])
        if mods == .command, editorUndo?.canUndo == true { editorUndo?.undo(); return true }
        if mods == [.command, .shift], editorUndo?.canRedo == true { editorUndo?.redo(); return true }
        return super.performKeyEquivalent(with: event)
    }
}

@MainActor
final class PathBarView: NSView, NSTextFieldDelegate, CrumbStripDelegate {
    private enum Mode { case rest, edit }

    private unowned let model: AppModel
    let paneID: PaneID
    let field = AddressField()
    private let strip = CrumbStripView()
    private let ghost = NSTextField(labelWithString: "")
    private let chip = ChipView()
    private let capsule = CAShapeLayer()
    private var tab: TabModel?
    private var handlers: [Int] = []
    private var observer: ObserverToken?
    private var mode = Mode.rest
    private var hovering = false
    private var lastLocation = ""

    // COMPLETING state
    private var completions = Completions()
    private lazy var panel: PopupPanel = {
        let p = PopupPanel(canBecomeKey: false, rowHeight: PathBarStyle.suggestionRowHeight)
        p.list.onHover = { [weak self] i in self?.panel.list.highlight = i }
        p.list.onActivate = { [weak self] i in self?.activateRow(i) }
        p.list.onDidDraw = { [weak self] in self?.listDidDraw() }
        p.list.setAccessibilityLabel("Suggestions")
        return p
    }()
    private lazy var folderMenu = FolderMenu(model: model) { [weak self] path in self?.goTo(path) }
    private var tabCycle: TabCycleAPI
    /// The text as typed, while ↓/↑ preview a row in the field.
    private var typedText: String?
    private var suppressChange = false
    private var lastText = ""
    /// "x doesn't exist" with Create Folder (Missing).
    private var offer: (path: String, nearest: String)?
    private var errorText: String?
    private var ending = false
    private var resignObserver: NSObjectProtocol?

    // Timing (DEBUG log + signposts)
    private static let signposter = OSSignposter(subsystem: "app.filestudio", category: "PathBar")
    private var pendingTiming: (start: TimeInterval, handlerStart: TimeInterval, completeMs: Double,
                                state: OSSignpostIntervalState)?

    init(model: AppModel, paneID: PaneID) {
        self.model = model
        self.paneID = paneID
        tabCycle = model.engine.makeTabCycle()
        super.init(frame: .zero)
        wantsLayer = true
        layer?.addSublayer(capsule)
        capsule.opacity = 0
        capsule.lineWidth = 1

        strip.delegate = self
        addSubview(strip)

        field.isBordered = false
        field.drawsBackground = false
        field.font = PathBarStyle.font
        field.focusRingType = .none
        field.lineBreakMode = .byClipping
        field.cell?.isScrollable = true
        field.cell?.wraps = false
        field.usesSingleLineMode = true
        field.delegate = self
        field.alphaValue = 0
        field.isHidden = true
        field.placeholderString = "Path or folder name"
        field.setAccessibilityLabel("Address")
        field.onBecomeFirstResponder = { [weak self] in
            guard let self, self.mode == .rest else { return }
            self.beginEdit(initial: nil)
        }
        addSubview(field)

        ghost.font = PathBarStyle.font
        ghost.textColor = .tertiaryLabelColor
        ghost.lineBreakMode = .byClipping
        ghost.isHidden = true
        ghost.setAccessibilityElement(false)
        addSubview(ghost)

        chip.isHidden = true
        addSubview(chip)

        setAccessibilityElement(false)

        handlers.append(model.registerShellHandler(for: "focusAddressBar") { [weak self] req, _ in
            guard let self, case .focusAddressBar(let pane, let text) = req, pane == self.paneID else { return false }
            self.beginEdit(initial: text.isEmpty ? nil : text)
            return true
        })
        handlers.append(model.registerShellHandler(for: "textField") { [weak self] req, _ in
            guard let self, self.isFieldFocused, case .textField(let cmd) = req else { return false }
            self.textCommand(cmd)
            return true
        })
        handlers.append(model.registerShellHandler(for: "offerCreateFolder") { [weak self] req, ctx in
            // Missing paths from elsewhere (Go to Folder…, scripts) for this pane.
            guard let self, let t = self.tab, ctx.tab == nil || ctx.tab == t.id,
                  case .offerCreateFolder(let path, let near) = req, self.window?.isKeyWindow == true,
                  self.isActivePane else { return false }
            self.beginEdit(initial: path)
            self.showOffer(path: path, nearest: near)
            return true
        })
        observer = model.observe { [weak self] e in self?.coreEvent(e) }
    }

    required init?(coder: NSCoder) { fatalError() }

    isolated deinit {
        handlers.forEach { model.unregisterShellHandler($0) }
        panel.dismiss()
    }

    /// Shows a tab's location (the pane's active tab).
    func show(_ t: TabModel) {
        if tab?.id != t.id, mode == .edit { cancelEdit(focusList: false) }
        tab = t
        field.coreTab = t.id
        refreshCrumbs()
    }

    private var isFieldFocused: Bool {
        guard mode == .edit, let tv = window?.firstResponder as? NSTextView, tv.isFieldEditor else { return false }
        return tv.delegate === field
    }

    /// True when this bar's pane holds the window's active tab.
    private var isActivePane: Bool {
        guard let t = tab, let w = model.window(of: t.id) else { return true }
        return w.activeTabID == t.id
    }

    private func coreEvent(_ e: CoreEvent) {
        switch e {
        case .listingChanged(let id, _, _) where id == tab?.id:
            if tab?.location != lastLocation { refreshCrumbs() }
        case .tabsChanged, .volumesChanged, .settingsChanged:
            refreshCrumbs()
        default:
            break
        }
    }

    private func refreshCrumbs() {
        guard let t = tab else { return }
        lastLocation = t.location
        strip.maxVisible = Int(model.settings.pathBar.maxCrumbs)
        strip.setCrumbs(t.api.breadcrumbs())
        updateCapsule()
    }

    // MARK: Layout and look

    private var capsuleRect: NSRect {
        let h = min(bounds.height, 28)
        return NSRect(x: 0, y: (bounds.height - h) / 2, width: bounds.width, height: h).insetBy(dx: 0.5, dy: 0.5)
    }

    override func layout() {
        super.layout()
        let c = capsuleRect
        capsule.frame = bounds
        capsule.path = CGPath(roundedRect: c, cornerWidth: c.height / 2, cornerHeight: c.height / 2, transform: nil)
        let inset = PathBarStyle.leadingInset
        strip.frame = NSRect(x: c.minX + inset - PathBarStyle.crumbPad, y: c.minY, width: c.width - inset, height: c.height)
        let showChip = !chip.isHidden && bounds.width > 320
        let chipW = showChip ? min(chip.fittingSize.width, bounds.width * 0.45) : 0
        let fh = ceil(field.intrinsicContentSize.height)
        field.frame = NSRect(x: c.minX + inset + 4, y: c.midY - fh / 2,
                             width: c.width - inset - 12 - (showChip ? chipW + 6 : 0), height: fh)
        chip.frame = NSRect(x: c.maxX - chipW - 4, y: c.midY - 9, width: chipW, height: 18)
        chip.alphaValue = showChip ? 1 : 0
        layoutGhost()
        if panel.isVisible { placePanel() }
    }

    override var isFlipped: Bool { true }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        updateCapsule(animated: false)
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        if let o = resignObserver { NotificationCenter.default.removeObserver(o) }
        resignObserver = nil
        guard let w = window else { return }
        // Losing the window's focus reverts an edit silently (HANDS-ON §8).
        resignObserver = NotificationCenter.default.addObserver(forName: NSWindow.didResignKeyNotification, object: w,
                                                                queue: .main) { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self, self.mode == .edit else { return }
                self.cancelEdit(focusList: false)
            }
        }
    }

    /// Flat at rest; a capsule on hover (QSpace) and while editing; red on error.
    private func updateCapsule(animated: Bool = true) {
        var stroke: NSColor?, fill: NSColor?, width: CGFloat = 1
        if mode == .edit {
            stroke = (offer != nil || errorText != nil) ? PathBarStyle.errorStroke : .keyboardFocusIndicatorColor
            fill = PathBarStyle.editFill
            width = 1.5
        } else if hovering || strip.openChevron != nil {
            stroke = PathBarStyle.hoverStroke
            fill = PathBarStyle.hoverFill
        }
        CATransaction.begin()
        CATransaction.setDisableActions(!animated)
        CATransaction.setAnimationDuration(PathBarStyle.fade)
        effectiveAppearance.performAsCurrentDrawingAppearance {
            if let stroke { self.capsule.strokeColor = stroke.cgColor }
            if let fill { self.capsule.fillColor = fill.cgColor }
        }
        capsule.lineWidth = width
        capsule.opacity = stroke == nil ? 0 : 1
        CATransaction.commit()
    }

    private var tracking: NSTrackingArea?
    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let t = tracking { removeTrackingArea(t) }
        let t = NSTrackingArea(rect: bounds, options: [.mouseEnteredAndExited, .activeAlways, .inVisibleRect, .cursorUpdate],
                               owner: self, userInfo: nil)
        addTrackingArea(t)
        tracking = t
    }
    override func mouseEntered(with event: NSEvent) { setHovering(true) }
    override func mouseExited(with event: NSEvent) { setHovering(false) }
    override func cursorUpdate(with event: NSEvent) {
        // An I-beam over the empty part (it edits); an arrow over crumbs.
        let p = strip.convert(event.locationInWindow, from: nil)
        if mode == .rest, strip.part(at: p) != nil { NSCursor.arrow.set() } else { NSCursor.iBeam.set() }
    }

    private func setHovering(_ h: Bool) {
        guard hovering != h else { return }
        hovering = h
        updateCapsule()
    }

    /// Debug automation: show the hover look.
    func debugSetHover(_ on: Bool, crumb: Int? = nil) {
        setHovering(on)
        strip.debugHover(crumb.map { .crumb($0) })
    }

    override func mouseDown(with event: NSEvent) {
        // Empty space (to the right of the crumbs) enters EDIT.
        if mode == .rest { beginEdit(initial: nil) }
    }

    // MARK: REST → EDIT

    /// The folder's path with a trailing `/` (HANDS-ON §8).
    private var editText: String {
        guard let loc = tab?.location else { return "/" }
        return loc == "/" ? "/" : loc + "/"
    }

    /// Enters EDIT: the full path, all selected; or `initial` (a typed `/` or
    /// `~`, a missing path) with the cursor at the end.
    func beginEdit(initial: String?) {
        guard tab != nil, let w = window else { return }
        if mode == .edit {
            // ⌘L while editing re-selects all. (Never makeFirstResponder on a field
            // whose editor is already active: AppKit re-enters and recurses.)
            if initial == nil {
                if !isFieldFocused { w.makeFirstResponder(field) }
                field.currentEditor()?.selectAll(nil)
                ghost.isHidden = true
            } else {
                setText(initial!, recomplete: true)
            }
            return
        }
        folderMenu.close(notify: false)
        mode = .edit
        offer = nil
        errorText = nil
        typedText = nil
        tabCycle.reset()
        suppressChange = true
        field.stringValue = initial ?? editText
        lastText = field.stringValue
        suppressChange = false
        field.textColor = .labelColor
        field.isHidden = false
        crossFade(toEdit: true)
        w.makeFirstResponder(field)
        if let ed = field.currentEditor() {
            if initial == nil { ed.selectAll(nil) } else { ed.selectedRange = NSRange(location: (ed.string as NSString).length, length: 0) }
        }
        updateCapsule()
        if initial != nil { refreshCompletions() }
        NSAccessibility.post(element: field, notification: .focusedUIElementChanged)
    }

    private func crossFade(toEdit: Bool) {
        let d = PathBarStyle.fade
        if toEdit { field.isHidden = false } else { strip.isHidden = false }
        NSAnimationContext.runAnimationGroup({ ctx in
            ctx.duration = d
            ctx.timingFunction = CAMediaTimingFunction(name: .easeInEaseOut)
            field.animator().alphaValue = toEdit ? 1 : 0
            strip.animator().alphaValue = toEdit ? 0 : 1
        }, completionHandler: { [weak self] in
            MainActor.assumeIsolated {
                guard let self else { return }
                if self.mode == .edit { self.strip.isHidden = true } else { self.field.isHidden = true }
            }
        })
    }

    // MARK: EDIT → REST

    /// Leaves EDIT without navigating: Esc (focus to the list) or focus loss.
    func cancelEdit(focusList: Bool) {
        guard mode == .edit else { return }
        finishEdit(focusList: focusList)
    }

    private func finishEdit(focusList: Bool) {
        guard mode == .edit, !ending else { return }
        ending = true
        defer { ending = false }
        mode = .rest
        closePanel()
        hideChip()
        offer = nil
        errorText = nil
        typedText = nil
        tabCycle.reset()
        ghost.isHidden = true
        if focusList {
            self.focusList()
        } else if isFieldFocused {
            window?.makeFirstResponder(nil)
        }
        suppressChange = true
        field.stringValue = ""
        suppressChange = false
        crossFade(toEdit: false)
        refreshCrumbs()
        updateCapsule()
    }

    /// Focus the file list of this pane (the nearest list view around the bar).
    private func focusList() {
        func find(_ v: NSView) -> NSView? {
            if let f = v as? CoreFocusReporting, f.coreFocus == .list, v.acceptsFirstResponder { return v }
            for s in v.subviews { if let r = find(s) { return r } }
            return nil
        }
        var v = superview
        while let cur = v {
            if let list = find(cur) { window?.makeFirstResponder(list); return }
            v = cur.superview
        }
        window?.makeFirstResponder(nil)
    }

    // MARK: Text changes → completions

    func controlTextDidChange(_ obj: Notification) {
        guard !suppressChange, mode == .edit else { return }
        let text = field.stringValue
        let inserted = Self.insertedText(old: lastText, new: text)
        lastText = text
        let undoing = field.currentEditor()?.undoManager?.isUndoing == true
            || field.currentEditor()?.undoManager?.isRedoing == true
        // The chip describes the last paste; any other edit retires it (⌘Z still works).
        hideChip()
        typedText = nil
        field.textColor = .labelColor
        tabCycle.reset()
        offer = nil
        errorText = nil
        updateCapsule()
        // A paste (or any multi-character insert): run the core's cleanup and
        // show what it changed. ⌘Z restores the original.
        if !undoing, inserted.count >= 2, cleanUpPaste(text) { return }
        refreshCompletions()
    }

    func controlTextDidEndEditing(_ obj: Notification) {
        // Focus went elsewhere: revert silently (HANDS-ON §8).
        guard mode == .edit, !ending else { return }
        finishEdit(focusList: false)
    }

    /// Keys the core didn't take (it normally does: these are fallbacks) and →.
    func control(_ control: NSControl, textView: NSTextView, doCommandBy sel: Selector) -> Bool {
        switch sel {
        case #selector(NSResponder.moveRight(_:)), #selector(NSResponder.moveToEndOfLine(_:)),
             #selector(NSResponder.moveToEndOfDocument(_:)):
            // → at the end of the text accepts the ghost text.
            let r = textView.selectedRange()
            if r.length == 0, r.location == (textView.string as NSString).length, !ghost.isHidden {
                acceptGhost()
                return true
            }
            return false
        case #selector(NSResponder.insertNewline(_:)): textCommand("addressbar.commit"); return true
        case #selector(NSResponder.cancelOperation(_:)): textCommand("addressbar.cancel"); return true
        case #selector(NSResponder.insertTab(_:)): textCommand("addressbar.complete_next"); return true
        case #selector(NSResponder.insertBacktab(_:)): textCommand("addressbar.complete_prev"); return true
        case #selector(NSResponder.moveDown(_:)): textCommand("addressbar.next"); return true
        case #selector(NSResponder.moveUp(_:)): textCommand("addressbar.prev"); return true
        default: return false
        }
    }

    /// The text a change inserted (the middle part that differs).
    static func insertedText(old: String, new: String) -> Substring {
        let o = Array(old), n = Array(new)
        var p = 0
        while p < o.count, p < n.count, o[p] == n[p] { p += 1 }
        var s = 0
        while s < o.count - p, s < n.count - p, o[o.count - 1 - s] == n[n.count - 1 - s] { s += 1 }
        let start = new.index(new.startIndex, offsetBy: p)
        let end = new.index(new.endIndex, offsetBy: -s)
        return new[start..<end]
    }

    /// UTF-8 offset of the insertion point (the core speaks bytes).
    private var cursorUTF8: Int {
        let s = field.stringValue
        guard let ed = field.currentEditor() else { return s.utf8.count }
        let r = ed.selectedRange
        let ns = s as NSString
        let end = min(r.location, ns.length)
        return ns.substring(to: end).utf8.count
    }

    /// Asks the core for completions and shows them (every keystroke).
    private func refreshCompletions() {
        guard let t = tab, mode == .edit else { return }
        let ev = NSApp.currentEvent
        let start = ev?.type == .keyDown ? ev!.timestamp : ProcessInfo.processInfo.systemUptime
        let state = Self.signposter.beginInterval("keystroke→list")
        let t0 = ProcessInfo.processInfo.systemUptime
        completions = t.api.complete(field.stringValue, cursor: UInt32(cursorUTF8))
        let completeMs = (ProcessInfo.processInfo.systemUptime - t0) * 1000
        pendingTiming = (start, t0, completeMs, state)
        showRows()
        // Draw now rather than at the next display pass, so the list is current
        // when the handler returns.
        if panel.isVisible { panel.list.displayIfNeeded() }
        finishTiming()
    }

    private func listDidDraw() { finishTiming() }

    private func finishTiming() {
        guard let p = pendingTiming else { return }
        pendingTiming = nil
        Self.signposter.endInterval("keystroke→list", p.state)
        #if DEBUG
        let now = ProcessInfo.processInfo.systemUptime
        print(String(format: "PATHBAR keystroke→list %.3f ms (handler %.3f ms, core complete %.3f ms, %d rows)",
                     (now - p.start) * 1000, (now - p.handlerStart) * 1000, p.completeMs, completions.items.count))
        fflush(stdout)
        #endif
    }

    // MARK: The suggestion list

    private func completionRow(_ c: Completion) -> PopupRow {
        var r = PopupRow(kind: .item, icon: CrumbIcons.folder(path: c.fullPath), title: c.displayName,
                         bold: Self.nsRanges(c.matchRanges, in: c.displayName), detail: c.breadcrumbText,
                         payload: c.fullPath)
        if c.isFrecent {
            r.badgeSymbol = "clock.arrow.circlepath"
            r.badgeText = c.lastVisit.map(Self.ago) ?? "Recent"
        } else {
            r.badgeSymbol = "arrow.turn.down.right"
        }
        return r
    }

    /// UTF-8 byte ranges → NSRanges of `s` (skips ranges that split a character).
    static func nsRanges(_ rs: [Range<Int>], in s: String) -> [NSRange] {
        let u = s.utf8
        return rs.compactMap { r in
            guard r.lowerBound >= 0, r.upperBound <= u.count,
                  let a = String.Index(u.index(u.startIndex, offsetBy: r.lowerBound), within: s),
                  let b = String.Index(u.index(u.startIndex, offsetBy: r.upperBound), within: s) else { return nil }
            return NSRange(a..<b, in: s)
        }
    }

    /// "now", "5 min", "3 h", "2 d" since a visit.
    static func ago(_ secs: UInt64) -> String {
        let d = max(0, Date().timeIntervalSince1970 - Double(secs))
        switch d {
        case ..<60: return "now"
        case ..<3600: return "\(Int(d / 60)) min"
        case ..<86_400: return "\(Int(d / 3600)) h"
        default: return "\(Int(d / 86_400)) d"
        }
    }

    private func showRows() {
        var rows: [PopupRow] = []
        if let o = offer {
            let home = model.engine.home()
            let tilde = { (p: String) in p == home || p.hasPrefix(home + "/") ? "~" + p.dropFirst(home.count) : p }
            rows.append(PopupRow(kind: .error, title: "“\(tilde(o.path))” doesn’t exist",
                                 detail: "Nearest folder: \(tilde(o.nearest))"))
            let missing = Self.missingCount(path: o.path, nearest: o.nearest)
            // The part that would be created ("x/y" when there are two).
            let name = o.path.hasPrefix(o.nearest + "/") ? String(o.path.dropFirst(o.nearest.count + 1))
                : (o.path as NSString).lastPathComponent
            rows.append(PopupRow(kind: .action, icon: PathBarStyle.symbol("folder.badge.plus", size: 13, color: .controlAccentColor),
                                 title: missing > 1 ? "Create \(missing) Folders “\(name)”" : "Create Folder “\(name)”",
                                 detail: missing > 1 ? tilde(o.path) : nil, badgeText: "↩", payload: "create"))
            rows.append(PopupRow(kind: .action, icon: CrumbIcons.folder(path: o.nearest),
                                 title: "Go to Nearest Folder", detail: tilde(o.nearest), payload: "nearest"))
        } else if let e = errorText {
            rows.append(PopupRow(kind: .error, title: e))
        } else {
            rows = completions.items.map(completionRow)
            if completions.skippedSlow {
                rows.append(PopupRow(kind: .note, title: "Slow folder: not listed", detail: "Type the full name"))
            }
        }
        let list = panel.list
        list.rows = rows
        if offer != nil {
            list.highlight = rows.firstIndex { $0.payload == "create" }
        } else if list.highlight == nil || typedText == nil {
            list.highlight = nil
        }
        layoutGhost()
        if rows.isEmpty {
            closePanel()
        } else {
            placePanel()
            if !panel.isVisible, let w = window { panel.present(over: w, makeKey: false) }
            list.needsDisplay = true
            announceList()
        }
    }

    /// Folders that "Create" would make (more than one: show the count).
    static func missingCount(path: String, nearest: String) -> Int {
        let rest = path.hasPrefix(nearest) ? String(path.dropFirst(nearest.count)) : path
        return max(1, rest.split(separator: "/").count)
    }

    private func placePanel() {
        guard let w = window else { return }
        let c = convert(capsuleRect, to: nil)
        let sr = w.convertToScreen(c)
        panel.place(topLeft: NSPoint(x: sr.minX, y: sr.minY - 3), width: min(max(sr.width, 360), 640))
    }

    private func closePanel() {
        guard panel.isVisible else { return }
        panel.dismiss()
        panel.list.highlight = nil
    }

    private var lastAnnounced = ""
    private func announceList() {
        let n = completions.items.count
        let text = offer != nil ? "Folder doesn’t exist. Create Folder available."
            : errorText ?? "\(n) \(n == 1 ? "suggestion" : "suggestions")"
        guard text != lastAnnounced else { return }
        lastAnnounced = text
        NSAccessibility.post(element: field, notification: .announcementRequested, userInfo: [
            .announcement: text, .priority: NSAccessibilityPriorityLevel.medium.rawValue,
        ])
    }

    // MARK: Ghost text

    private func layoutGhost() {
        let g = completions.ghostText
        guard mode == .edit, offer == nil, errorText == nil, typedText == nil, !g.isEmpty,
              let ed = field.currentEditor(), ed.selectedRange.length == 0,
              ed.selectedRange.location == (field.stringValue as NSString).length else {
            ghost.isHidden = true
            return
        }
        let typedW = (field.stringValue as NSString).size(withAttributes: [.font: PathBarStyle.font]).width
        // The field's text and the ghost label both start 2 pt (line fragment
        // padding) inside their frames, so the ghost frame starts at the typed width.
        let x = field.frame.minX + typedW
        let gw = ceil((g as NSString).size(withAttributes: [.font: PathBarStyle.font]).width) + 4
        guard x + 8 < field.frame.maxX else { ghost.isHidden = true; return }
        ghost.stringValue = g
        ghost.frame = NSRect(x: x, y: field.frame.minY, width: min(gw, field.frame.maxX - x), height: field.frame.height)
        ghost.isHidden = false
    }

    private func acceptGhost() {
        let g = completions.ghostText
        guard !g.isEmpty else {
            field.currentEditor()?.selectedRange = NSRange(location: (field.stringValue as NSString).length, length: 0)
            return
        }
        setText(field.stringValue + g, recomplete: true)
    }

    // MARK: Commands (from the core's keymap)

    private func textCommand(_ cmd: String) {
        guard mode == .edit else { return }
        switch cmd {
        case "addressbar.commit": commit(otherPane: false)
        case "addressbar.commit_other_pane":
            if offer != nil { createFolder() } else { commit(otherPane: true) }
        case "addressbar.cancel":
            if panel.isVisible {
                // First Esc closes the list; the text (or the previewed row) stays.
                if typedText != nil { typedText = nil; field.textColor = .labelColor }
                closePanel()
                ghost.isHidden = true
            } else {
                cancelEdit(focusList: true)
            }
        case "addressbar.complete_next", "addressbar.complete_prev":
            tab(backwards: cmd == "addressbar.complete_prev")
        case "addressbar.next": moveHighlight(1)
        case "addressbar.prev": moveHighlight(-1)
        case "addressbar.accept_ghost": acceptGhost()
        case "addressbar.delete_component":
            let (text, cur) = model.engine.deletePathComponent(field.stringValue, cursor: cursorUTF8)
            setText(text, recomplete: true, cursorUTF8: cur)
        case "addressbar.clear_line":
            // Deletes back to the start; with a selection, deletes the selection (text-field rules).
            let s = field.stringValue as NSString
            let r = field.currentEditor()?.selectedRange ?? NSRange(location: s.length, length: 0)
            if r.length > 0, NSMaxRange(r) <= s.length {
                let text = s.replacingCharacters(in: r, with: "")
                setText(text, recomplete: true, cursorUTF8: (text as NSString).substring(to: r.location).utf8.count)
            } else {
                setText(s.substring(from: min(r.location, s.length)), recomplete: true, cursorUTF8: 0)
            }
        case "addressbar.add_favourite":
            let path = highlightedCompletion?.fullPath ?? tab?.location ?? ""
            model.perform(Invocation(id: cmd, arg: .path(path), tab: tab?.id), via: .keyboard, window: windowID)
            showChip(model.statusMessage ?? "Added to Favourites", undoable: false)
        default:
            break
        }
    }

    private var windowID: WindowID? { tab.flatMap { model.window(of: $0.id)?.id } }

    /// Replaces the text (cursor at the end, or at a UTF-8 offset).
    private func setText(_ text: String, recomplete: Bool, cursorUTF8: Int? = nil) {
        suppressChange = true
        field.stringValue = text
        lastText = text
        suppressChange = false
        typedText = nil
        field.textColor = .labelColor
        tabCycle.reset()
        offer = nil
        errorText = nil
        hideChip()
        if let ed = field.currentEditor() {
            let loc: Int
            if let c = cursorUTF8, let i = text.utf8.index(text.utf8.startIndex, offsetBy: min(c, text.utf8.count), limitedBy: text.utf8.endIndex) {
                loc = NSRange(text.startIndex..<(String.Index(i, within: text) ?? text.endIndex), in: text).length
            } else {
                loc = (text as NSString).length
            }
            ed.selectedRange = NSRange(location: loc, length: 0)
            // Keep the insertion point in view (lay out the new text first).
            if let tv = ed as? NSTextView {
                tv.layoutManager?.ensureLayout(for: tv.textContainer!)
                tv.scrollRangeToVisible(tv.selectedRange())
            }
        }
        updateCapsule()
        if recomplete { refreshCompletions() }
    }

    /// Tab / ⇧Tab through the core's state machine.
    private func tab(backwards: Bool) {
        if !panel.isVisible, offer == nil { refreshCompletions() }
        switch tabCycle.press(completions, backwards: backwards) {
        case .nothing:
            NSSound.beep()
        case .extend(let newInput):
            setText(newInput, recomplete: true)
        case .select(let i, let text):
            suppressChange = true
            field.stringValue = text
            lastText = text
            suppressChange = false
            field.currentEditor()?.selectedRange = NSRange(location: (text as NSString).length, length: 0)
            typedText = nil
            panel.list.highlight = i
            panel.list.revealHighlight()
            ghost.isHidden = true
        }
    }

    private var highlightedCompletion: Completion? {
        guard offer == nil, errorText == nil, let h = panel.list.highlight, completions.items.indices.contains(h) else { return nil }
        return completions.items[h]
    }

    /// ↓/↑: move the highlight; it previews the row in the field (grey).
    /// ↑ from the first row returns to the typed text.
    private func moveHighlight(_ d: Int) {
        if !panel.isVisible {
            if d > 0 { refreshCompletions() }
            return
        }
        let list = panel.list
        let selectable = list.rows.indices.filter { list.rows[$0].isSelectable }
        guard !selectable.isEmpty else { return }
        let cur = list.highlight.flatMap { selectable.firstIndex(of: $0) }
        var next: Int?
        switch (cur, d > 0) {
        case (nil, true): next = 0
        case (nil, false): next = nil
        case (let c?, true): next = min(c + 1, selectable.count - 1)
        case (let c?, false): next = c == 0 ? nil : c - 1
        }
        list.highlight = next.map { selectable[$0] }
        list.revealHighlight()
        if offer != nil { return }
        if let h = list.highlight, completions.items.indices.contains(h) {
            if typedText == nil { typedText = field.stringValue }
            suppressChange = true
            field.stringValue = completions.items[h].insertText
            lastText = field.stringValue
            suppressChange = false
            field.textColor = .secondaryLabelColor
            field.currentEditor()?.selectedRange = NSRange(location: (field.stringValue as NSString).length, length: 0)
            ghost.isHidden = true
            announceRow(h)
        } else if let typed = typedText {
            suppressChange = true
            field.stringValue = typed
            lastText = typed
            suppressChange = false
            typedText = nil
            field.textColor = .labelColor
            layoutGhost()
        }
    }

    private func announceRow(_ i: Int) {
        let rows = panel.list.rows
        guard rows.indices.contains(i) else { return }
        NSAccessibility.post(element: field, notification: .announcementRequested, userInfo: [
            .announcement: "\(rows[i].accessibilityText), \(i + 1) of \(rows.count)",
            .priority: NSAccessibilityPriorityLevel.high.rawValue,
        ])
    }

    /// A click on a row, or Return with it highlighted.
    private func activateRow(_ i: Int) {
        let rows = panel.list.rows
        guard rows.indices.contains(i) else { return }
        switch rows[i].payload {
        case "create" where offer != nil: createFolder()
        case "nearest" where offer != nil: commitPath(offer!.nearest, otherPane: false)
        default:
            if completions.items.indices.contains(i) { commitPath(completions.items[i].fullPath, otherPane: false) }
        }
    }

    // MARK: Return

    private func commit(otherPane: Bool) {
        if let h = panel.list.highlight, panel.isVisible, panel.list.rows.indices.contains(h),
           panel.list.rows[h].isSelectable {
            if offer != nil {
                activateRow(h)
            } else if completions.items.indices.contains(h) {
                commitPath(completions.items[h].fullPath, otherPane: otherPane)
            }
            return
        }
        // With the error showing and nothing highlighted, Return keeps the message.
        if offer != nil { return }
        commitPath(field.stringValue, otherPane: otherPane)
    }

    /// Goes through the core's `parse_location` (`addressbar.commit`).
    private func commitPath(_ text: String, otherPane: Bool) {
        guard let t = tab else { return }
        let id = otherPane ? "addressbar.commit_other_pane" : "addressbar.commit"
        do {
            let out = try model.engine.perform(Invocation(id: id, arg: .path(text), tab: t.id), via: .keyboard)
            if case .offerCreateFolder(let path, let near)? = out.shell {
                showOffer(path: path, nearest: near)
                return
            }
            var shown = out
            shown.shell = nil
            model.handle(shown, in: windowID)
            if otherPane {
                // ⌘Return keeps focus in the bar.
                field.currentEditor()?.selectAll(nil)
                closePanel()
            } else {
                finishEdit(focusList: true)
            }
        } catch let e as CoreError {
            showError(e.message)
        } catch {
            showError(error.localizedDescription)
        }
    }

    /// Missing: keep EDIT, outline the bar, offer Create Folder (HANDS-ON §8).
    private func showOffer(path: String, nearest: String) {
        offer = (path, nearest)
        errorText = nil
        typedText = nil
        field.textColor = .labelColor
        ghost.isHidden = true
        updateCapsule()
        showRows()
        NSAccessibility.post(element: field, notification: .announcementRequested, userInfo: [
            .announcement: "\((path as NSString).lastPathComponent) doesn’t exist. Return creates the folder.",
            .priority: NSAccessibilityPriorityLevel.high.rawValue,
        ])
    }

    private func showError(_ message: String) {
        offer = nil
        errorText = message
        ghost.isHidden = true
        updateCapsule()
        showRows()
        NSSound.beep()
    }

    private func createFolder() {
        guard let t = tab, let o = offer else { return }
        do {
            let out = try model.engine.createFolderAndGo(tab: t.id, path: o.path)
            model.handle(out, in: windowID)
            finishEdit(focusList: true)
        } catch let e as CoreError {
            showError(e.message)
        } catch {
            showError(error.localizedDescription)
        }
    }

    // MARK: Paste cleanup chip

    /// Runs the core's cleanup on pasted text; if it changed anything, puts
    /// the cleaned path in the field (undoably) and shows the chip.
    private func cleanUpPaste(_ raw: String) -> Bool {
        guard let t = tab else { return false }
        let p = model.engine.parseLocation(raw, cwd: t.location)
        guard let chipText = p.chip else { return false }
        let cleaned: String
        switch p.location {
        case .dir(let d): cleaned = d == "/" ? "/" : d + "/"
        case .file(let d, let n): cleaned = (d as NSString).appendingPathComponent(n)
        case .missing(let path, _): cleaned = path
        case .remote(let u): cleaned = u
        }
        guard cleaned != raw, field.currentEditor() is NSTextView else { return false }
        // Replace in the next run-loop turn, so the paste and the cleanup are
        // separate undo steps: ⌘Z brings the raw text back.
        // (A timer, not `async`: the undo manager closes its group when the run
        // loop waits, and main-queue blocks run before that.)
        Timer.scheduledTimer(withTimeInterval: 0.001, repeats: false) { [weak self] _ in
            MainActor.assumeIsolated { self?.applyCleanup(raw: raw, cleaned: cleaned, chip: chipText) }
        }
        return true
    }

    private func applyCleanup(raw: String, cleaned: String, chip chipText: String, tries: Int = 0) {
        guard mode == .edit, field.stringValue == raw, let tv = field.currentEditor() as? NSTextView else { return }
        watchUndo(tv.undoManager)
        // Close the text view's open typing group, so the paste stays its own step;
        // if the paste's own undo group is still open, wait for it to close.
        tv.breakUndoCoalescing()
        if let um = tv.undoManager, um.groupingLevel > 0, tries < 50 {
            Timer.scheduledTimer(withTimeInterval: 0.01, repeats: false) { [weak self] _ in
                MainActor.assumeIsolated {
                    self?.applyCleanup(raw: raw, cleaned: cleaned, chip: chipText, tries: tries + 1)
                }
            }
            return
        }
        swapCleanup(to: cleaned, from: raw, showing: chipText, chip: chipText, undo: tv.undoManager)
    }

    /// Puts `new` in the field as one undo step whose undo puts `old` back
    /// (and whose redo cleans up again, chip included).
    private func swapCleanup(to new: String, from old: String, showing: String?, chip: String, undo um: UndoManager?) {
        guard mode == .edit, let tv = field.currentEditor() as? NSTextView else { return }
        um?.registerUndo(withTarget: self) { bar in
            MainActor.assumeIsolated {
                bar.swapCleanup(to: old, from: new, showing: showing == nil ? chip : nil, chip: chip, undo: um)
            }
        }
        um?.setActionName("Clean Up Paste")
        suppressChange = true
        tv.string = new
        suppressChange = false
        lastText = new
        typedText = nil
        field.textColor = .labelColor
        tabCycle.reset()
        if let c = showing { showChip(c, undoable: true) } else { hideChip() }
        // The chip narrows the field: lay out first, then keep the end in view.
        layoutSubtreeIfNeeded()
        tv.selectedRange = NSRange(location: (new as NSString).length, length: 0)
        tv.scrollRangeToVisible(tv.selectedRange)
        refreshCompletions()
    }

    private var undoObservers: [NSObjectProtocol] = []
    private weak var watchedUndo: UndoManager?

    /// Undo/redo in the field editor change the text without our change
    /// handler; resync (and drop the chip once its change is undone).
    private func watchUndo(_ um: UndoManager?) {
        guard let um, um !== watchedUndo else { return }
        undoObservers.forEach(NotificationCenter.default.removeObserver)
        watchedUndo = um
        undoObservers = [NSNotification.Name.NSUndoManagerDidUndoChange, .NSUndoManagerDidRedoChange].map { name in
            NotificationCenter.default.addObserver(forName: name, object: um, queue: .main) { [weak self] _ in
                MainActor.assumeIsolated {
                    guard let self, self.mode == .edit, self.lastText != self.field.stringValue else { return }
                    self.hideChip()
                    self.lastText = self.field.stringValue
                    self.typedText = nil
                    self.field.textColor = .labelColor
                    self.tabCycle.reset()
                    self.refreshCompletions()
                }
            }
        }
    }

    private var chipTimer: Timer?

    private func showChip(_ text: String, undoable: Bool) {
        chip.set(text: text, hint: undoable ? "⌘Z" : nil)
        chip.isHidden = false
        needsLayout = true
        chipTimer?.invalidate()
        chipTimer = Timer.scheduledTimer(withTimeInterval: 6, repeats: false) { [weak self] _ in
            MainActor.assumeIsolated { self?.hideChip() }
        }
        NSAccessibility.post(element: field, notification: .announcementRequested, userInfo: [
            .announcement: text, .priority: NSAccessibilityPriorityLevel.medium.rawValue,
        ])
    }

    private func hideChip() {
        chipTimer?.invalidate()
        chipTimer = nil
        guard !chip.isHidden else { return }
        chip.isHidden = true
        needsLayout = true
    }

    // MARK: Crumbs (CrumbStripDelegate)

    private func goTo(_ path: String) {
        guard let t = tab else { return }
        do {
            let out = try t.api.go(path)
            model.handle(out, in: windowID)
        } catch let e as CoreError {
            if e.kind == .notAvailable { NSSound.beep() }
            model.statusMessage = e.message
        } catch {}
    }

    func crumbStrip(go crumb: Crumb) { goTo(crumb.path) }

    func crumbStripEnterEdit() { beginEdit(initial: nil) }

    func crumbStrip(contextMenuFor crumb: Crumb) -> NSMenu? {
        guard let t = tab, let items = try? model.engine.contextMenu(
            .crumb(tab: t.id, path: crumb.path), context: MenuContext(pasteboardHasFiles: Pasteboard.hasFiles())) else { return nil }
        return ContextMenuBuilder.menu(items, model: model)
    }

    func crumbStrip(openSubfoldersOf index: Int, anchor: NSRect) {
        guard let w = window, strip.crumbs.indices.contains(index) else { return }
        let c = strip.crumbs[index]
        let next = strip.crumbs.indices.contains(index + 1) ? strip.crumbs[index + 1].path : nil
        strip.openChevron = .chevron(index)
        folderMenu.onClose = { [weak self] in
            self?.strip.openChevron = nil
            self?.updateCapsule()
        }
        updateCapsule()
        folderMenu.open(subfoldersOf: c.path, select: next, anchor: anchor, in: w)
    }

    func crumbStrip(openOverflow hidden: [Crumb], anchor: NSRect) {
        guard let w = window, !hidden.isEmpty else { return }
        strip.openChevron = .overflow
        folderMenu.onClose = { [weak self] in self?.strip.openChevron = nil }
        folderMenu.open(crumbs: hidden, anchor: anchor, in: w)
    }

    func crumbStrip(dropAction sources: [String], onto dir: String) -> DropAction {
        model.engine.dropAction(sources: sources, destDir: dir, mods: Mods(NSEvent.modifierFlags))
    }

    func crumbStrip(drop sources: [String], onto dir: String) -> Bool {
        do {
            let out = try model.engine.dropFiles(sources: sources, destDir: dir, mods: Mods(NSEvent.modifierFlags))
            model.handle(out, in: windowID)
            return true
        } catch let e as CoreError {
            model.statusMessage = e.message
            return false
        } catch { return false }
    }

    func crumbStrip(springLoad crumb: Crumb) { goTo(crumb.path) }

    func crumbStrip(hovering: Bool) {}
}

/// The transient "Cleaned up paste: removed quotes  ⌘Z" chip.
@MainActor
final class ChipView: NSView {
    private let label = NSTextField(labelWithString: "")
    private let hint = NSTextField(labelWithString: "")

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.cornerRadius = 9
        for l in [label, hint] {
            l.font = .systemFont(ofSize: 11)
            l.lineBreakMode = .byTruncatingTail
            l.translatesAutoresizingMaskIntoConstraints = false
            addSubview(l)
        }
        label.textColor = .controlAccentColor
        hint.textColor = .secondaryLabelColor
        hint.setContentCompressionResistancePriority(.required, for: .horizontal)
        label.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        NSLayoutConstraint.activate([
            label.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 8),
            label.centerYAnchor.constraint(equalTo: centerYAnchor),
            hint.leadingAnchor.constraint(equalTo: label.trailingAnchor, constant: 6),
            hint.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -8),
            hint.centerYAnchor.constraint(equalTo: centerYAnchor),
        ])
        setAccessibilityElement(true)
        setAccessibilityRole(.staticText)
    }
    required init?(coder: NSCoder) { fatalError() }

    func set(text: String, hint h: String?) {
        label.stringValue = text
        hint.stringValue = h ?? ""
        hint.isHidden = h == nil
        setAccessibilityLabel(h == nil ? text : "\(text). Command Z undoes it.")
    }

    override func updateLayer() {
        layer?.backgroundColor = NSColor.controlAccentColor.withAlphaComponent(0.14).cgColor
    }
    override var wantsUpdateLayer: Bool { true }
}



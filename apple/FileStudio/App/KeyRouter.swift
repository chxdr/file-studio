// KeyRouter.swift — every key press goes to the core first (F-001, F-006).
//
// A local event monitor sees key-down events before menus and views. It asks
// the focused view what kind of focus it is (`CoreFocusReporting`), sends the
// chord to `handle_key`, and swallows the event when the core took it. When
// the core says "not mine" (typing in the address bar, say) the event goes
// on to the focused control and the menus as usual.

import AppKit

/// Adopted by views that take keyboard focus, so the router can tell the
/// core where a key was pressed. The router walks up from the first
/// responder (and from a field editor to its text field) to the first view
/// that adopts this.
@MainActor
public protocol CoreFocusReporting: AnyObject {
    /// What kind of focus this is.
    var coreFocus: Focus { get }
    /// The tab this view belongs to (its pane's active tab).
    var coreTab: TabID? { get }
}

@MainActor
final class KeyRouter {
    private var monitor: Any?
    private unowned let model: AppModel

    init(model: AppModel) { self.model = model }

    func install() {
        monitor = NSEvent.addLocalMonitorForEvents(matching: [.keyDown]) { [weak self] e in
            guard let self else { return e }
            return self.route(e) ? nil : e
        }
    }

    /// The focus context for the key window's first responder.
    static func context(in window: NSWindow?) -> (InputContext, WindowID?) {
        let wid = (window?.windowController as? CoreWindowIdentifying)?.coreWindowID
        var r: NSResponder? = window?.firstResponder
        // A field editor reports for the text field it edits.
        if let tv = r as? NSTextView, tv.isFieldEditor, let owner = tv.delegate as? NSResponder { r = owner }
        while let cur = r {
            if let f = cur as? CoreFocusReporting { return (InputContext(focus: f.coreFocus, tab: f.coreTab), wid) }
            r = cur.nextResponder
        }
        return (InputContext(focus: .other, tab: nil), wid)
    }

    private func route(_ e: NSEvent) -> Bool {
        guard let chord = KeyChord(event: e), let w = e.window ?? NSApp.keyWindow,
              w.windowController is CoreWindowIdentifying, w.attachedSheet == nil else { return false }
        let (ctx, wid) = Self.context(in: w)
        return model.handleKey(chord, context: ctx, window: wid)
    }
}

/// Adopted by window controllers that show a core window.
@MainActor
public protocol CoreWindowIdentifying: AnyObject {
    var coreWindowID: WindowID { get }
}

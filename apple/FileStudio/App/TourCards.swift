// TourCards.swift — the power tour's eight cards: what each says and which
// keys it shows. Keys are never written here: each card names commands, and
// their key caps are read from the core's *current* bindings
// (`engine.commandKeys`), so rebinding a command (or switching to the Vim
// profile) updates the tour. Only typed text ("gig") and gestures ("hold ⌘")
// are literal.

import Foundation

/// One power-tour card.
struct TourCard: Identifiable, Equatable {
    /// What the card's keys row shows, in order.
    enum Key: Equatable {
        /// Every current binding of a registered command, joined with "or".
        case command(String, caption: String)
        /// Literal text the user types (shown as a text chip, not a key cap).
        case typed(String, caption: String)
        /// A gesture or key that is not a command (hold ⌘).
        case literal(String, caption: String)
    }

    /// The drawn demo each card shows (`TourDemos.swift`).
    enum Demo: Equatable { case addressBar, pasteCleanup, dualPane, queue, cutPaste, typeSelect, overlay, rebind }

    let id: String
    /// Short name for the step pill.
    let short: String
    let symbol: String
    let title: String
    /// Two lines at most. `{command.id}` stands for that command's current
    /// first key (see `text(keysFor:)`), so the prose follows rebinding too.
    let body: String
    let keys: [Key]
    let demo: Demo
    /// What the demo shows, for VoiceOver.
    let demoDescription: String

    static let all: [TourCard] = [
        TourCard(
            id: "address", short: "Address", symbol: "character.cursor.ibeam",
            title: "The address bar is always ready",
            body: "Press a key, type a path, and {addressbar.complete_next} completes it. A bare word jumps to the folder you use most that matches — “gig” finds your gig photos.",
            keys: [.command("nav.address_bar", caption: "edit the path"),
                   .command("addressbar.complete_next", caption: "complete"),
                   .typed("gig", caption: "jump by name"),
                   .command("addressbar.commit", caption: "go")],
            demo: .addressBar,
            demoDescription: "The address bar: typing gig suggests the folder 2026-11-11_gig, then goes there."),
        TourCard(
            id: "paste", short: "Paste", symbol: "doc.on.clipboard",
            title: "Paste any path, even a messy one",
            body: "Quotes, file:// links, %20s and stray spaces are cleaned up as you paste ({edit.undo} puts them back). A folder that isn’t there yet? {addressbar.commit} offers Create Folder.",
            keys: [.command("nav.address_bar", caption: "edit the path"),
                   .command("edit.paste", caption: "paste"),
                   .command("addressbar.commit", caption: "go or create")],
            demo: .pasteCleanup,
            demoDescription: "A pasted file URL with quotes and percent signs becomes a clean path with a Cleaned up chip, then a Create Folder offer for a folder that doesn’t exist."),
        TourCard(
            id: "dual", short: "Panes", symbol: "rectangle.split.2x1",
            title: "Two panes, one keystroke apart",
            body: "Open a second pane, then copy or move the selection straight across. {pane.switch} switches which pane has the keyboard.",
            keys: [.command("pane.toggle_dual", caption: "dual pane"),
                   .command("file.copy_to_other_pane", caption: "copy across"),
                   .command("file.move_to_other_pane", caption: "move across"),
                   .command("pane.switch", caption: "switch pane")],
            demo: .dualPane,
            demoDescription: "Two panes side by side; a file is copied from the left pane to the right one, then focus switches panes."),
        TourCard(
            id: "queue", short: "Queue", symbol: "list.bullet.rectangle",
            title: "Copies queue up, and you stay in charge",
            body: "Pause, resume or reorder operations. On a clash, a sheet asks what to do; Replace sends the old item to the Trash. Everything can be undone.",
            keys: [.command("edit.undo", caption: "undo"),
                   .command("edit.redo", caption: "redo")],
            demo: .queue,
            demoDescription: "The operation queue: one copy paused, two reordered, then a conflict sheet offering Replace, which moves the old item to the Trash."),
        TourCard(
            id: "cut", short: "Cut", symbol: "scissors",
            title: "Cut and paste to move",
            body: "Cut marks the selection to move; nothing is deleted. Paste in another folder and the items move there, safely, through the queue.",
            keys: [.command("edit.cut", caption: "mark to move"),
                   .command("edit.paste", caption: "move here")],
            demo: .cutPaste,
            demoDescription: "A file is cut and shows dimmed, still in place; pasting in another folder moves it there."),
        TourCard(
            id: "find", short: "Find", symbol: "line.3.horizontal.decrease.circle",
            title: "Type to select, or filter",
            body: "Just type a name to jump to it, as in Finder. The filter narrows the list to what matches, and {filter.clear} clears it.",
            keys: [.typed("rep", caption: "jump to “Report”"),
                   .command("filter.focus", caption: "filter"),
                   .command("filter.clear", caption: "clear")],
            demo: .typeSelect,
            demoDescription: "On the left, typing rep selects Report.pdf in the full list. On the right, the filter rep shows only the matching files."),
        TourCard(
            id: "overlay", short: "Hold ⌘", symbol: "command",
            title: "Hold ⌘ to see what you can press",
            body: "Hold a modifier for a moment and the shortcuts it unlocks appear. Use the mouse for something with a key, and a “Did you know?” hint may say so.",
            keys: [.literal("hold ⌘", caption: "shortcut overlay"),
                   .command("palette.open", caption: "every command")],
            demo: .overlay,
            demoDescription: "Holding Command shows a panel of Command shortcuts; a Did you know hint suggests a key for something done with the mouse."),
        TourCard(
            id: "rebind", short: "Yours", symbol: "keyboard",
            title: "Every key is yours to change",
            body: "Rebind anything in Settings › Shortcuts, or switch to the Vim profile. This tour always shows your current keys.",
            keys: [.command("app.settings", caption: "Settings"),
                   .command("palette.open", caption: "every command")],
            demo: .rebind,
            demoDescription: "A shortcut row being rebound from F5 to a new key, and the Standard and Vim profiles."),
    ]
}

extension TourCard {
    /// The body with each `{command.id}` replaced by that command's current
    /// first key, or by its title in quotes when it has none.
    func text(keysFor: (String) -> [String], titleFor: (String) -> String) -> String {
        var out = ""
        var rest = Substring(body)
        while let open = rest.firstIndex(of: "{"), let close = rest[open...].firstIndex(of: "}") {
            out += rest[..<open]
            let id = String(rest[rest.index(after: open)..<close])
            out += keysFor(id).first ?? "“\(titleFor(id))”"
            rest = rest[rest.index(after: close)...]
        }
        return out + rest
    }
}

/// Key-cap text helpers (pure, no I/O).
enum TourKeys {
    /// The key-cap strings a key row shows; empty when a command has no binding.
    static func caps(_ key: TourCard.Key, keysFor: (String) -> [String]) -> [String] {
        switch key {
        case .command(let id, _):
            // The same keys can be bound in two contexts (⎋ in the list and in the filter).
            Array(keysFor(id).reduce(into: [String]()) { if !$0.contains($1) { $0.append($1) } }.prefix(3))
        case .typed(let t, _), .literal(let t, _): [t]
        }
    }

    /// How VoiceOver reads a key-cap string: "⇧⌘P" → "Shift Command P".
    static func spoken(_ caps: String) -> String {
        let names: [Character: String] = [
            "⌘": "Command", "⇧": "Shift", "⌥": "Option", "⌃": "Control", "⇥": "Tab", "↩": "Return",
            "⎋": "Escape", "⌫": "Delete", "⌦": "Forward Delete", "↑": "Up Arrow", "↓": "Down Arrow",
            "←": "Left Arrow", "→": "Right Arrow", "/": "Slash", "\\": "Backslash", "~": "Tilde",
            "↖": "Home", "↘": "End", ",": "Comma", ".": "Period",
        ]
        var words: [String] = []
        var plain = ""
        for ch in caps {
            if let n = names[ch] {
                if !plain.isEmpty { words.append(plain); plain = "" }
                words.append(n)
            } else if ch == " " {
                if !plain.isEmpty { words.append(plain); plain = "" }
                words.append("then")
            } else {
                plain.append(ch)
            }
        }
        if !plain.isEmpty { words.append(plain) }
        return words.joined(separator: " ")
    }
}

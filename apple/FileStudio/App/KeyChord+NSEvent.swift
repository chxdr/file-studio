// KeyChord+NSEvent.swift — translate an NSEvent into the core's neutral KeyChord.
// Pure translation: no decision about what a key means is made here.

import AppKit

extension KeyChord {
    /// `nil` for events that aren't a key the core knows (a bare modifier,
    /// a dead key, an IME composition).
    init?(event e: NSEvent) {
        let f = e.modifierFlags
        let key: Key
        switch e.keyCode {
        case 123: key = .left
        case 124: key = .right
        case 125: key = .down
        case 126: key = .up
        case 49: key = .space
        case 36: key = .returnKey
        case 76: key = .enter
        case 53: key = .escape
        case 51: key = .delete
        case 117: key = .forwardDelete
        case 48: key = .tab
        case 115: key = .home
        case 119: key = .end
        case 116: key = .pageUp
        case 121: key = .pageDown
        case 122: key = .f(1)
        case 120: key = .f(2)
        case 99: key = .f(3)
        case 118: key = .f(4)
        case 96: key = .f(5)
        case 97: key = .f(6)
        case 98: key = .f(7)
        case 100: key = .f(8)
        case 101: key = .f(9)
        case 109: key = .f(10)
        case 103: key = .f(11)
        case 111: key = .f(12)
        default:
            // The character as typed WITHOUT Shift (so ⇧3 is '3', not '#').
            let raw = e.characters(byApplyingModifiers: []) ?? e.charactersIgnoringModifiers
            guard let ch = raw?.lowercased().first, !ch.isNewline, ch.unicodeScalars.allSatisfy({
                !CharacterSet.controlCharacters.contains($0) && !(0xF700...0xF8FF).contains($0.value)
            }) else { return nil }
            key = .char(ch)
        }
        self.init(key: key, mods: Mods(shift: f.contains(.shift), cmd: f.contains(.command),
                                       alt: f.contains(.option), ctrl: f.contains(.control)))
    }
}

extension Mods {
    init(_ f: NSEvent.ModifierFlags) {
        self.init(shift: f.contains(.shift), cmd: f.contains(.command), alt: f.contains(.option), ctrl: f.contains(.control))
    }
}

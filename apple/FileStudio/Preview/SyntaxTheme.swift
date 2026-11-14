// SyntaxTheme.swift — colours for the core's highlight spans. The core says
// what a token is (keyword, string…); this picks a colour for light or dark.

import SwiftUI
import AppKit

enum SyntaxTheme {
    private static func dynamic(_ light: UInt32, _ dark: UInt32) -> Color {
        func ns(_ v: UInt32) -> NSColor {
            NSColor(srgbRed: CGFloat((v >> 16) & 0xff) / 255, green: CGFloat((v >> 8) & 0xff) / 255,
                    blue: CGFloat(v & 0xff) / 255, alpha: 1)
        }
        return Color(nsColor: NSColor(name: nil) { a in
            a.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua ? ns(dark) : ns(light)
        })
    }

    /// The colour for a token class; `nil` keeps the normal text colour.
    static func color(_ c: TokenClass) -> Color? {
        switch c {
        case .keyword: dynamic(0xAD3DA4, 0xFC5FA3)
        case .string: dynamic(0xC41A16, 0xFC6A5D)
        case .number: dynamic(0x1C00CF, 0xD0BF69)
        case .comment: dynamic(0x5D6C79, 0x7F8C98)
        case .function: dynamic(0x0F68A0, 0x41A1C0)
        case .type: dynamic(0x3E8087, 0x5DD8FF)
        case .constant: dynamic(0x6C36A9, 0xA167E6)
        case .punctuation: .secondary
        case .tag: dynamic(0xAD3DA4, 0xFC5FA3)
        case .attribute: dynamic(0x836C28, 0xD9C97C)
        case .heading: dynamic(0x0F68A0, 0x6BDFFF)
        case .variable: nil
        case .invalid: .red
        }
    }

    /// One line, coloured. Span offsets are UTF-16 code units.
    static func attributed(_ line: PreviewTextLine, font: Font) -> AttributedString {
        var a = AttributedString(line.text)
        a.font = font
        let u = line.text.utf16
        for s in line.spans {
            guard let color = color(s.kind),
                  let lo = u.index(u.startIndex, offsetBy: s.start, limitedBy: u.endIndex),
                  let hi = u.index(lo, offsetBy: s.len, limitedBy: u.endIndex),
                  let alo = AttributedString.Index(lo, within: a), let ahi = AttributedString.Index(hi, within: a),
                  alo < ahi else { continue }
            a[alo..<ahi].foregroundColor = color
            if s.kind == .heading { a[alo..<ahi].font = font.bold() }
        }
        return a
    }
}

// UsageTables.swift — the usage view's two tables: the size-sorted tree list
// (rows, expansion and keyboard moves come from the core) and the breakdown
// by file type, whose colour swatches double as the treemap's legend.

import AppKit

/// A table that hands its keys to the window controller first (the core
/// decides what ←/→/↑/↓ do) and reports right-clicked rows.
final class UsageTableView: NSTableView {
    var onKey: ((NSEvent) -> Bool)?
    var onMenu: ((Int, NSEvent) -> NSMenu?)?

    override func keyDown(with e: NSEvent) {
        if onKey?(e) != true { super.keyDown(with: e) }
    }

    override func menu(for e: NSEvent) -> NSMenu? {
        let row = self.row(at: convert(e.locationInWindow, from: nil))
        guard row >= 0 else { return nil }
        return onMenu?(row, e)
    }
}

/// The percent-of-parent bar: a rounded track with a fill and the figure.
final class UsageBarView: NSView {
    var fraction: Double = 0 { didSet { needsDisplay = true } }
    var text = "" { didSet { needsDisplay = true } }
    var emphasized = false { didSet { needsDisplay = true } }

    override func draw(_ dirty: NSRect) {
        let barW = max(40, bounds.width - 52)
        let track = NSRect(x: 2, y: (bounds.height - 8) / 2, width: barW, height: 8)
        NSColor.quaternaryLabelColor.setFill()
        NSBezierPath(roundedRect: track, xRadius: 4, yRadius: 4).fill()
        let w = max(0, min(1, fraction)) * track.width
        if w > 0.5 {
            (emphasized ? NSColor.white.withAlphaComponent(0.85) : NSColor.controlAccentColor).setFill()
            NSBezierPath(roundedRect: NSRect(x: track.minX, y: track.minY, width: max(w, 3), height: track.height),
                         xRadius: 4, yRadius: 4).fill()
        }
        let attrs: [NSAttributedString.Key: Any] = [
            .font: NSFont.monospacedDigitSystemFont(ofSize: 11, weight: .regular),
            .foregroundColor: emphasized ? NSColor.white : NSColor.secondaryLabelColor,
        ]
        let s = NSAttributedString(string: text, attributes: attrs)
        let sz = s.size()
        s.draw(at: NSPoint(x: bounds.width - sz.width - 2, y: (bounds.height - sz.height) / 2))
    }

    override var isFlipped: Bool { false }
}

/// The name cell: indent, disclosure triangle, icon, name and a status glyph.
final class UsageNameCell: NSTableCellView {
    let disclosure = NSButton()
    let icon = NSImageView()
    let label = NSTextField(labelWithString: "")
    let badge = NSImageView()
    private var leading: NSLayoutConstraint!
    var onToggle: (() -> Void)?

    override init(frame: NSRect) {
        super.init(frame: frame)
        disclosure.bezelStyle = .disclosure
        disclosure.setButtonType(.pushOnPushOff)
        disclosure.title = ""
        disclosure.target = self
        disclosure.action = #selector(toggle)
        disclosure.refusesFirstResponder = true
        icon.imageScaling = .scaleProportionallyDown
        label.lineBreakMode = .byTruncatingMiddle
        label.font = .systemFont(ofSize: 13)
        badge.imageScaling = .scaleProportionallyDown
        badge.contentTintColor = .secondaryLabelColor
        for v in [disclosure, icon, label, badge] {
            v.translatesAutoresizingMaskIntoConstraints = false
            addSubview(v)
        }
        textField = label
        imageView = icon
        leading = disclosure.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 2)
        NSLayoutConstraint.activate([
            leading,
            disclosure.centerYAnchor.constraint(equalTo: centerYAnchor),
            disclosure.widthAnchor.constraint(equalToConstant: 14),
            icon.leadingAnchor.constraint(equalTo: disclosure.trailingAnchor, constant: 2),
            icon.centerYAnchor.constraint(equalTo: centerYAnchor),
            icon.widthAnchor.constraint(equalToConstant: 16),
            icon.heightAnchor.constraint(equalToConstant: 16),
            label.leadingAnchor.constraint(equalTo: icon.trailingAnchor, constant: 5),
            label.centerYAnchor.constraint(equalTo: centerYAnchor),
            badge.leadingAnchor.constraint(greaterThanOrEqualTo: label.trailingAnchor, constant: 4),
            badge.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -4),
            badge.centerYAnchor.constraint(equalTo: centerYAnchor),
            badge.widthAnchor.constraint(equalToConstant: 14),
        ])
        label.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    @objc private func toggle() { onToggle?() }

    @MainActor
    func show(_ r: UsageRow, path: String?) {
        leading.constant = 2 + CGFloat(r.depth) * 14
        disclosure.isHidden = !r.expandable
        disclosure.state = r.expanded ? .on : .off
        icon.image = IconCache.shared.icon(key: r.iconKey, path: path)
        label.stringValue = r.name
        label.textColor = r.otherVolume ? .secondaryLabelColor : .labelColor
        let symbol: String? = r.denied ? "lock.fill" : r.inCloud ? "icloud" : r.otherVolume ? "externaldrive" : nil
        badge.image = symbol.flatMap { NSImage(systemSymbolName: $0, accessibilityDescription: r.note) }
        badge.isHidden = symbol == nil
        toolTip = r.note.isEmpty ? nil : r.note
        setAccessibilityLabel("\(r.name), \(r.sizeText)")
    }
}

/// One row of the type breakdown: swatch, extension, kind, size, count, share.
final class UsageTypeCell: NSTableCellView {
    let swatch = NSImageView()
    let label = NSTextField(labelWithString: "")

    override init(frame: NSRect) {
        super.init(frame: frame)
        label.lineBreakMode = .byTruncatingTail
        for v in [swatch, label] {
            v.translatesAutoresizingMaskIntoConstraints = false
            addSubview(v)
        }
        textField = label
        NSLayoutConstraint.activate([
            swatch.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 4),
            swatch.centerYAnchor.constraint(equalTo: centerYAnchor),
            swatch.widthAnchor.constraint(equalToConstant: 10),
            swatch.heightAnchor.constraint(equalToConstant: 10),
            label.leadingAnchor.constraint(equalTo: swatch.trailingAnchor, constant: 6),
            label.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -2),
            label.centerYAnchor.constraint(equalTo: centerYAnchor),
        ])
    }

    required init?(coder: NSCoder) { fatalError("not used") }
}

/// A plain right- or left-aligned text cell with monospaced digits.
@MainActor
func usageTextCell(_ table: NSTableView, id: String, right: Bool, secondary: Bool) -> NSTableCellView {
    let ident = NSUserInterfaceItemIdentifier(id)
    if let c = table.makeView(withIdentifier: ident, owner: nil) as? NSTableCellView { return c }
    let c = NSTableCellView()
    c.identifier = ident
    let t = NSTextField(labelWithString: "")
    t.font = .monospacedDigitSystemFont(ofSize: 12, weight: .regular)
    t.alignment = right ? .right : .left
    t.textColor = secondary ? .secondaryLabelColor : .labelColor
    t.lineBreakMode = .byTruncatingTail
    t.translatesAutoresizingMaskIntoConstraints = false
    c.addSubview(t)
    c.textField = t
    NSLayoutConstraint.activate([
        t.leadingAnchor.constraint(equalTo: c.leadingAnchor, constant: 2),
        t.trailingAnchor.constraint(equalTo: c.trailingAnchor, constant: -4),
        t.centerYAnchor.constraint(equalTo: c.centerYAnchor),
    ])
    return c
}

/// Builds a table column.
@MainActor
func usageColumn(_ id: String, _ title: String, width: CGFloat, min: CGFloat, right: Bool = false) -> NSTableColumn {
    let c = NSTableColumn(identifier: NSUserInterfaceItemIdentifier(id))
    c.title = title
    c.width = width
    c.minWidth = min
    if right { c.headerCell.alignment = .right }
    return c
}

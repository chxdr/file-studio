// RenamePreviewTable.swift — the preview list: old name → new name, with a
// badge per row. An NSTableView that asks the session for the visible rows
// only (F-005), cached until the preview's version changes, so 10,000 items
// cost nothing extra per keystroke.

import AppKit
import SwiftUI

struct RenamePreviewTable: NSViewRepresentable {
    let session: RenameSessionAPI
    let summary: RenameSummary

    func makeCoordinator() -> Coordinator { Coordinator(session: session) }

    func makeNSView(context: Context) -> NSScrollView {
        let table = NSTableView()
        table.style = .inset
        table.usesAlternatingRowBackgroundColors = false
        table.rowHeight = 22
        table.intercellSpacing = NSSize(width: 6, height: 0)
        table.allowsColumnReordering = false
        table.allowsMultipleSelection = false
        table.columnAutoresizingStyle = .uniformColumnAutoresizingStyle
        table.setAccessibilityLabel("Preview")
        for (id, title, width) in [("status", "", 22.0), ("old", "Name", 220.0), ("new", "New Name", 320.0),
                                   ("folder", "Folder", 110.0), ("note", "", 160.0)] {
            let c = NSTableColumn(identifier: NSUserInterfaceItemIdentifier(id))
            c.title = title
            c.width = width
            c.minWidth = id == "status" ? 22 : 60
            if id == "status" { c.maxWidth = 22; c.resizingMask = [] }
            table.addTableColumn(c)
        }
        table.dataSource = context.coordinator
        table.delegate = context.coordinator
        context.coordinator.table = table
        let scroll = NSScrollView()
        scroll.documentView = table
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        scroll.borderType = .noBorder
        return scroll
    }

    func updateNSView(_ scroll: NSScrollView, context: Context) {
        context.coordinator.update(summary)
    }

    @MainActor
    final class Coordinator: NSObject, NSTableViewDataSource, NSTableViewDelegate {
        let session: RenameSessionAPI
        weak var table: NSTableView?
        private var version: UInt64 = .max
        private var total = 0
        private var cache: [Int: RenameRow] = [:]

        init(session: RenameSessionAPI) { self.session = session }

        func update(_ s: RenameSummary) {
            guard s.version != version || Int(s.total) != total else { return }
            version = s.version
            total = Int(s.total)
            cache.removeAll(keepingCapacity: true)
            table?.tableColumn(withIdentifier: .init("folder"))?.isHidden = !s.manyFolders
            table?.reloadData()
        }

        func row(_ i: Int) -> RenameRow? {
            if let r = cache[i] { return r }
            // Fetch the block around the row (the visible rows, more or less).
            let start = max(0, i - 16)
            for r in session.rows(start: UInt32(start), count: 96) { cache[Int(r.index)] = r }
            if cache.count > 4000 { cache = cache.filter { abs($0.key - i) < 200 } }
            return cache[i]
        }

        func numberOfRows(in tableView: NSTableView) -> Int { total }

        func tableView(_ tableView: NSTableView, viewFor col: NSTableColumn?, row: Int) -> NSView? {
            guard let col, let r = self.row(row) else { return nil }
            let id = col.identifier
            if id.rawValue == "status" {
                let v = (tableView.makeView(withIdentifier: id, owner: nil) as? NSImageView) ?? {
                    let iv = NSImageView()
                    iv.identifier = id
                    iv.imageScaling = .scaleProportionallyDown
                    return iv
                }()
                let (symbol, tint, label): (String, NSColor, String) = switch r.status {
                case .problem: ("exclamationmark.octagon.fill", .systemRed, "Problem")
                case .unchanged: ("equal.circle", .tertiaryLabelColor, "Unchanged")
                case .ready where !r.warning.isEmpty: ("exclamationmark.triangle.fill", .systemYellow, "Warning")
                case .ready: ("checkmark.circle.fill", .systemGreen, "Will be renamed")
                }
                v.image = NSImage(systemSymbolName: symbol, accessibilityDescription: label)
                v.contentTintColor = tint
                v.toolTip = [r.message, r.warning].filter { !$0.isEmpty }.joined(separator: "\n")
                return v
            }
            let tf = (tableView.makeView(withIdentifier: id, owner: nil) as? NSTextField) ?? {
                let t = NSTextField(labelWithString: "")
                t.identifier = id
                t.lineBreakMode = id.rawValue == "note" ? .byTruncatingTail : .byTruncatingMiddle
                t.font = id.rawValue == "note" || id.rawValue == "folder"
                    ? .systemFont(ofSize: NSFont.smallSystemFontSize) : .systemFont(ofSize: NSFont.systemFontSize)
                return t
            }()
            switch id.rawValue {
            case "old":
                tf.stringValue = r.oldName
                tf.textColor = .secondaryLabelColor
            case "new":
                tf.stringValue = r.newName
                tf.textColor = switch r.status {
                case .problem: .systemRed
                case .unchanged: .tertiaryLabelColor
                case .ready: .labelColor
                }
            case "folder":
                tf.stringValue = r.folder
                tf.textColor = .secondaryLabelColor
            default:
                tf.stringValue = r.status == .problem ? r.message : r.warning
                tf.textColor = r.status == .problem ? .systemRed : .secondaryLabelColor
            }
            tf.toolTip = tf.stringValue
            return tf
        }
    }
}

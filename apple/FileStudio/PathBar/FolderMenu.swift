// FolderMenu.swift — the menu a crumb's chevron (or the "…" overflow
// button) opens: a folder's subfolders, from `engine.subfolders` (HANDS-ON §8).
//
// A cascading popup rather than NSMenu, because NSMenu can't both go to a
// folder on click/Return and open its subfolders on →. Keys: ↑/↓ move,
// Return goes, → opens the highlighted folder's subfolders, ← closes a
// level, Esc closes, typing selects by name prefix (1 s reset), Home/End.

import AppKit

@MainActor
final class FolderMenu {
    private struct Level {
        let panel: PopupPanel
        let path: String?
    }

    private unowned let model: AppModel
    private var levels: [Level] = []
    private let onGo: (String) -> Void
    private var typed = ""
    private var typedAt = Date.distantPast
    private weak var parentWindow: NSWindow?
    /// Called once when the whole menu has closed.
    var onClose: (() -> Void)?
    private(set) var isOpen = false

    init(model: AppModel, onGo: @escaping (String) -> Void) {
        self.model = model
        self.onGo = onGo
    }

    /// Rows for a folder's subfolders.
    private func rows(subfoldersOf path: String) -> [PopupRow] {
        let subs = model.engine.subfolders(path: path)
        if subs.isEmpty {
            return [PopupRow(kind: .note, icon: nil, title: "No Subfolders")]
        }
        return subs.map(row)
    }

    private func row(_ c: Crumb) -> PopupRow {
        PopupRow(kind: .item, icon: CrumbIcons.icon(c), title: c.name, hasSubmenu: true, payload: c.path)
    }

    /// Opens the subfolders of `path` under `anchor` (screen rect), highlighting
    /// the subfolder named `select` (the next crumb), if any.
    func open(subfoldersOf path: String, select: String?, anchor: NSRect, in window: NSWindow) {
        open(rows: rows(subfoldersOf: path), title: path, select: select, anchor: anchor, in: window)
    }

    /// Opens a list of crumbs (the hidden middle crumbs of an overflowing bar).
    func open(crumbs: [Crumb], anchor: NSRect, in window: NSWindow) {
        open(rows: crumbs.map(row), title: "Hidden folders", select: nil, anchor: anchor, in: window)
    }

    private func open(rows: [PopupRow], title: String, select: String?, anchor: NSRect, in window: NSWindow) {
        close(notify: false)
        parentWindow = window
        isOpen = true
        let p = makePanel(rows: rows, path: title)
        p.list.highlight = rows.firstIndex { $0.payload == select } ?? rows.firstIndex(where: \.isSelectable)
        p.list.setAccessibilityLabel("Subfolders of \((title as NSString).lastPathComponent)")
        levels = [Level(panel: p, path: title)]
        p.place(topLeft: NSPoint(x: anchor.minX - 6, y: anchor.minY - 2), width: width(for: rows))
        p.present(over: window, makeKey: true)
        p.list.revealHighlight()
        #if DEBUG
        print("MENU folder menu: \(title)")
        for r in rows { print("MENU   \(r.title)\(r.hasSubmenu ? "  ›" : "")\(r.kind == .note ? "  (disabled)" : "")") }
        #endif
        announce(rows)
    }

    private func width(for rows: [PopupRow]) -> CGFloat {
        let w = rows.map { ($0.title as NSString).size(withAttributes: [.font: PathBarStyle.font]).width }.max() ?? 100
        return min(max(w + 80, 180), 420)
    }

    private func makePanel(rows: [PopupRow], path: String?) -> PopupPanel {
        let p = PopupPanel(canBecomeKey: true, rowHeight: PathBarStyle.menuRowHeight)
        p.list.rows = rows
        p.list.onHover = { [weak p] i in p?.list.highlight = i }
        p.list.onActivate = { [weak self, weak p] i in
            guard let self, let p, p.list.rows.indices.contains(i) else { return }
            self.go(p.list.rows[i].payload)
        }
        p.list.onOpenSubmenu = { [weak self, weak p] i in
            guard let self, let p, let lvl = self.levels.firstIndex(where: { $0.panel === p }) else { return }
            p.list.highlight = i
            self.openSubmenu(level: lvl)
        }
        p.onKey = { [weak self, weak p] e in
            guard let self, let p, let lvl = self.levels.firstIndex(where: { $0.panel === p }) else { return false }
            return self.key(e, level: lvl)
        }
        p.onResignKey = { [weak self] in
            // Closing when focus leaves the menu (a click elsewhere), not when
            // it moves between our own levels.
            DispatchQueue.main.async {
                guard let self, self.isOpen else { return }
                if !self.levels.contains(where: { $0.panel.isKeyWindow }) { self.close() }
            }
        }
        return p
    }

    private func openSubmenu(level: Int) {
        guard levels.indices.contains(level) else { return }
        let lp = levels[level].panel
        guard let h = lp.list.highlight, lp.list.rows[h].hasSubmenu else { return }
        while levels.count > level + 1 { levels.removeLast().panel.dismiss() }
        let path = lp.list.rows[h].payload
        let rows = rows(subfoldersOf: path)
        let p = makePanel(rows: rows, path: path)
        p.list.highlight = rows.firstIndex(where: \.isSelectable)
        p.list.setAccessibilityLabel("Subfolders of \((path as NSString).lastPathComponent)")
        levels.append(Level(panel: p, path: path))
        let rowRect = lp.convertToScreen(lp.list.convert(lp.list.rect(ofRow: h), to: nil))
        p.place(topLeft: NSPoint(x: lp.frame.maxX - 4, y: rowRect.maxY + 4), width: width(for: rows))
        p.present(over: parentWindow ?? lp, makeKey: true)
        announce(rows)
    }

    private func key(_ e: NSEvent, level: Int) -> Bool {
        let list = levels[level].panel.list
        let selectable = list.rows.indices.filter { list.rows[$0].isSelectable }
        func move(_ d: Int) {
            guard !selectable.isEmpty else { return }
            let cur = list.highlight.flatMap { selectable.firstIndex(of: $0) } ?? (d > 0 ? -1 : selectable.count)
            list.highlight = selectable[max(0, min(selectable.count - 1, cur + d))]
            list.revealHighlight()
        }
        switch e.keyCode {
        case 125: move(1)
        case 126: move(-1)
        case 115, 116: list.highlight = selectable.first; list.revealHighlight()
        case 119, 121: list.highlight = selectable.last; list.revealHighlight()
        case 36, 76:
            if let h = list.highlight, list.rows[h].isSelectable { go(list.rows[h].payload) }
        case 124: openSubmenu(level: level)
        case 123:
            if level > 0 {
                levels.removeLast().panel.dismiss()
                levels[level - 1].panel.makeKey()
            }
        case 53:
            if level > 0 {
                levels.removeLast().panel.dismiss()
                levels[level - 1].panel.makeKey()
            } else {
                close()
            }
        default:
            guard let ch = e.charactersIgnoringModifiers, !ch.isEmpty, e.modifierFlags.intersection([.command, .control]).isEmpty
            else { return false }
            typeSelect(ch, list: list)
        }
        return true
    }

    /// Type-to-select inside the menu (case- and accent-insensitive prefix).
    private func typeSelect(_ s: String, list: PopupListView) {
        if Date().timeIntervalSince(typedAt) > 1 { typed = "" }
        typedAt = Date()
        typed += s
        let key = Self.fold(typed)
        if let i = list.rows.indices.first(where: { list.rows[$0].isSelectable && Self.fold(list.rows[$0].title).hasPrefix(key) }) {
            list.highlight = i
            list.revealHighlight()
        }
    }

    private static func fold(_ s: String) -> String {
        s.folding(options: [.caseInsensitive, .diacriticInsensitive, .widthInsensitive], locale: nil)
    }

    private func go(_ path: String) {
        guard !path.isEmpty else { return }
        close()
        onGo(path)
    }

    /// Closes every level and gives focus back to the window.
    func close(notify: Bool = true) {
        guard isOpen || !levels.isEmpty else { return }
        isOpen = false
        for l in levels { l.panel.dismiss() }
        levels = []
        parentWindow?.makeKey()
        if notify { onClose?() }
    }

    private func announce(_ rows: [PopupRow]) {
        guard let p = levels.last?.panel else { return }
        let n = rows.filter(\.isSelectable).count
        NSAccessibility.post(element: p.list, notification: .announcementRequested, userInfo: [
            .announcement: "\(p.list.accessibilityLabel() ?? "Menu"), \(n) \(n == 1 ? "folder" : "folders")",
            .priority: NSAccessibilityPriorityLevel.high.rawValue,
        ])
    }
}

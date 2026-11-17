// TourDemoParts.swift — the pieces the power-tour demos are built from. They
// are the app's own views, fed a small fixed dataset instead of a tab:
//
//   rows and cells        FileRowView / FileCellView in a table with the
//                         app's FileHeaderView, columns and row height
//   pane header           PaneHeaderView (glyph, address bar, subtitle, 3 pt bar)
//   address bar           CrumbStripView at rest; in EDIT the same field font,
//                         capsule (PathBarView.capsuleStyle) and ChipView
//   suggestions           PopupListView in the popover-material box PopupPanel uses,
//                         with rows from PathBarView.completionRow / offerRows
//   active pane           PaneOutlineView
//
// A live pane is not possible here: AppModel is one per process (it routes
// every core event to itself), so the tour can't run a second, fake one.
// Nothing here reads the disk or talks to the core; the demos never touch files.

import AppKit
import SwiftUI

// MARK: - Fixed demo data

/// The demo folders and files (not on disk).
@MainActor
enum TourDemoData {
    static let home = "/Users/you"

    /// A list row as the core would format it (kind "Folder" or "PDF File").
    static func row(_ name: String, folder: Bool = false, size: String = "--", kind: String? = nil,
                    modified: String = "Yesterday at 16:02", selected: Bool = false, cursor: Bool = false,
                    cut: Bool = false) -> Row {
        let ext = (name as NSString).pathExtension.lowercased()
        let kindText = kind ?? (folder ? "Folder" : ext.isEmpty ? "Document" : "\(ext.uppercased()) File")
        return Row(index: 0, name: name, kind: folder ? .folder : .file,
                   iconKey: folder ? "folder" : (ext.isEmpty ? "file" : "file.\(ext)"), kindText: kindText, size: 0,
                   sizeText: folder ? "--" : size, modifiedNs: 0, modifiedText: modified, isSelected: selected,
                   isCursor: cursor, isCutPending: cut)
    }

    /// Crumbs for a path under the demo home ("~" is the home crumb).
    static func crumbs(_ names: [String]) -> [Crumb] {
        var out = [Crumb(name: "Macintosh HD", path: "/", kind: .volumeRoot, isCurrent: false),
                   Crumb(name: "Users", path: "/Users", kind: .folder, isCurrent: false),
                   Crumb(name: "you", path: home, kind: .home, isCurrent: names.isEmpty)]
        var p = home
        for (i, n) in names.enumerated() {
            p += "/" + n
            out.append(Crumb(name: n, path: p, kind: .folder, isCurrent: i == names.count - 1))
        }
        return out
    }

    static func subtitle(_ n: Int, free: String = "412.6 GB") -> String { "\(PaneText.items(UInt32(n))), \(free) available" }
}

// MARK: - Specs (what a demo frame shows)

/// The address bar at one moment.
enum DemoBar: Equatable {
    /// Crumbs (flat, ForkLift style).
    case rest([Crumb])
    /// The field: text, whether it is all selected, the caret (blinking), the
    /// paste chip, and a red outline (a folder that doesn't exist).
    case edit(text: String, selected: Bool = false, caret: Bool = true, chip: String? = nil, error: Bool = false)
}

/// A suggestion list under the bar.
struct DemoPopup: Equatable {
    var rows: [PopupRow]
    var highlight: Int?
}

/// One pane at one moment.
struct DemoPaneSpec: Equatable {
    var crumbs: [Crumb]
    var bar: DemoBar
    var subtitle: String
    var rows: [Row]
    var columns: [(ColumnID, CGFloat)] = [(.name, 0), (.size, 70), (.modified, 140), (.kind, 110)]
    /// Dual pane and this one is active: the outline and the header bar.
    var marked = false
    /// The list has the keyboard: selection in the accent colour.
    var focused = true
    var popup: DemoPopup?
    /// The status line: left text, then a message (the active pane's).
    var status: String = ""
    var message: String?
    /// Free space on the folder's volume (subtitle and status line).
    var free = "412.6 GB"

    static func == (a: DemoPaneSpec, b: DemoPaneSpec) -> Bool {
        a.crumbs == b.crumbs && a.bar == b.bar && a.subtitle == b.subtitle && a.rows == b.rows
            && a.columns.map(\.0) == b.columns.map(\.0) && a.columns.map(\.1) == b.columns.map(\.1)
            && a.marked == b.marked && a.focused == b.focused && a.popup == b.popup && a.status == b.status
            && a.message == b.message && a.free == b.free
    }
}

// MARK: - SwiftUI wrapper

/// One or two real panes side by side, with the window's 1 pt divider.
struct DemoWindow: NSViewRepresentable {
    let panes: [DemoPaneSpec]

    func makeNSView(context: Context) -> DemoWindowView { DemoWindowView() }
    func updateNSView(_ v: DemoWindowView, context: Context) { v.apply(panes) }
}

/// Hosts a real SwiftUI piece that has buttons and menus of its own (the
/// conflict sheet, the hint) as a picture: it never takes clicks, focus or
/// key equivalents, so the tour's own keys keep working. (Pieces must also
/// leave out default/cancel buttons: AppKit sends Return and Esc to a
/// window's default button directly — see ConflictCard.drawingOnly.)
struct KeyInert<Content: View>: NSViewRepresentable {
    let content: Content

    init(@ViewBuilder _ content: () -> Content) { self.content = content() }

    func makeNSView(context: Context) -> KeyInertHostingView<Content> {
        let v = KeyInertHostingView(rootView: content)
        v.sizingOptions = [.intrinsicContentSize]
        return v
    }
    func updateNSView(_ v: KeyInertHostingView<Content>, context: Context) { v.rootView = content }
}

final class KeyInertHostingView<Content: View>: NSHostingView<Content> {
    override func performKeyEquivalent(with event: NSEvent) -> Bool { false }
    override var acceptsFirstResponder: Bool { false }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }
}

// MARK: - AppKit views

/// The panes, laid out like PaneSplitView (a hairline between them).
@MainActor
final class DemoWindowView: NSView {
    private var panes: [DemoPaneView] = []
    private var dividers: [NSBox] = []

    override var isFlipped: Bool { true }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    func apply(_ specs: [DemoPaneSpec]) {
        while panes.count < specs.count {
            let p = DemoPaneView()
            addSubview(p)
            panes.append(p)
        }
        while panes.count > specs.count { panes.removeLast().removeFromSuperview() }
        while dividers.count < max(specs.count - 1, 0) {
            let d = NSBox()
            d.boxType = .separator
            addSubview(d)
            dividers.append(d)
        }
        while dividers.count > max(specs.count - 1, 0) { dividers.removeLast().removeFromSuperview() }
        for (p, s) in zip(panes, specs) { p.apply(s) }
        needsLayout = true
    }

    override func layout() {
        super.layout()
        let n = CGFloat(max(panes.count, 1))
        let w = (bounds.width - (n - 1)) / n
        for (i, p) in panes.enumerated() {
            p.frame = NSRect(x: CGFloat(i) * (w + 1), y: 0, width: w, height: bounds.height)
        }
        for (i, d) in dividers.enumerated() {
            d.frame = NSRect(x: CGFloat(i + 1) * w + CGFloat(i), y: 0, width: 1, height: bounds.height)
        }
        // The suggestion list floats over the panes, like its child panel.
        for p in panes { p.placePopup(in: self) }
    }
}

/// One pane: header (glyph, address bar, subtitle), list, status line, outline.
@MainActor
final class DemoPaneView: NSView {
    private let header = PaneHeaderView()
    private let bar = DemoAddressBar()
    private let list = DemoListView()
    private let status = NSHostingView(rootView: DemoStatusLine(left: "", message: nil, free: ""))
    private let outline = PaneOutlineView()
    private let popup = DemoPopupView()
    private var spec: DemoPaneSpec?

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        for v in [header, list, status, outline] as [NSView] { addSubview(v) }
        header.install(pathBar: bar)
        status.sizingOptions = []
    }
    required init?(coder: NSCoder) { fatalError() }

    override var isFlipped: Bool { true }

    func apply(_ s: DemoPaneSpec) {
        guard s != spec else { return }
        let old = spec
        spec = s
        if old?.crumbs.last?.path != s.crumbs.last?.path {
            header.glyph.image = s.crumbs.last.map { $0.kind == .home ? IconCache.shared.icon(key: "folder") : FolderIcon.icon(for: $0.path) }
        }
        header.subtitle.stringValue = s.subtitle
        header.setActive(s.marked)
        if case .edit = s.bar { header.setEditing(true) } else { header.setEditing(false) }
        bar.apply(s.bar)
        list.update(rows: s.rows, active: s.focused, columns: s.columns)
        status.rootView = DemoStatusLine(left: s.status, message: s.message, free: s.free)
        outline.isHidden = !s.marked
        popup.apply(s.popup)
        needsLayout = true
    }

    override func layout() {
        super.layout()
        let b = bounds
        header.frame = NSRect(x: 0, y: 0, width: b.width, height: PaneHeaderView.height)
        status.frame = NSRect(x: 0, y: b.height - PaneStatusLine.height, width: b.width, height: PaneStatusLine.height)
        list.frame = NSRect(x: 0, y: header.frame.maxY, width: b.width, height: status.frame.minY - header.frame.maxY)
        outline.frame = b
    }

    /// Puts the suggestion list under the address capsule (as PathBarView.placePanel:
    /// 3 pt below it, at least 360 pt wide), in `host`'s coordinates.
    func placePopup(in host: NSView) {
        guard popup.hasRows else { popup.removeFromSuperview(); return }
        if popup.superview !== host { host.addSubview(popup) }
        let c = bar.convert(bar.capsuleRect, to: host)
        let w = min(max(c.width, 360), host.bounds.maxX - c.minX)
        popup.frame = NSRect(x: c.minX, y: c.maxY + 3, width: w, height: popup.contentHeight)
    }
}

/// The address bar's two looks, with the same parts and layout as
/// PathBarView (crumbs at rest; the field, capsule and chip in EDIT).
@MainActor
final class DemoAddressBar: NSView {
    private let strip = CrumbStripView()
    private let field = NSTextField(labelWithString: "")
    private let caret = NSView()
    private let chip = ChipView()
    private let capsule = CAShapeLayer()
    private var state: DemoBar = .rest([])

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.addSublayer(capsule)
        field.font = PathBarStyle.font
        field.lineBreakMode = .byClipping
        field.isHidden = true
        caret.wantsLayer = true
        caret.isHidden = true
        chip.isHidden = true
        for v in [strip, field, caret, chip] as [NSView] { addSubview(v) }
    }
    required init?(coder: NSCoder) { fatalError() }

    override var isFlipped: Bool { true }

    func apply(_ s: DemoBar) {
        state = s
        switch s {
        case .rest(let crumbs):
            strip.isHidden = false
            strip.setCrumbs(crumbs)
            field.isHidden = true
            caret.isHidden = true
            chip.isHidden = true
        case .edit(let text, let selected, let showCaret, let chipText, _):
            strip.isHidden = true
            field.isHidden = false
            let a = NSMutableAttributedString(string: text, attributes: [.font: PathBarStyle.font,
                                                                         .foregroundColor: NSColor.labelColor])
            if selected { a.addAttribute(.backgroundColor, value: NSColor.selectedTextBackgroundColor,
                                         range: NSRange(location: 0, length: a.length)) }
            field.attributedStringValue = a
            caret.isHidden = !showCaret || selected
            if let chipText {
                chip.set(text: chipText, hint: "⌘Z")
                chip.isHidden = false
            } else {
                chip.isHidden = true
            }
        }
        updateCapsule()
        needsLayout = true
    }

    /// PathBarView.capsuleRect.
    var capsuleRect: NSRect {
        let h = min(bounds.height, 28)
        return NSRect(x: 0, y: (bounds.height - h) / 2, width: bounds.width, height: h).insetBy(dx: 0.5, dy: 0.5)
    }

    private func updateCapsule() {
        let (stroke, fill, width): (NSColor?, NSColor?, CGFloat)
        if case .edit(_, _, _, _, let error) = state {
            (stroke, fill, width) = PathBarView.capsuleStyle(editing: true, error: error, hovering: false)
        } else {
            (stroke, fill, width) = PathBarView.capsuleStyle(editing: false, error: false, hovering: false)
        }
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        effectiveAppearance.performAsCurrentDrawingAppearance {
            self.capsule.strokeColor = stroke?.cgColor
            self.capsule.fillColor = fill?.cgColor
            self.caret.layer?.backgroundColor = NSColor.textInsertionPointColor.cgColor
        }
        capsule.lineWidth = width
        capsule.opacity = stroke == nil ? 0 : 1
        CATransaction.commit()
    }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        updateCapsule()
    }

    /// PathBarView.layout, for the parts the demo shows.
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
        // The field's text starts 2 pt inside an editable field's frame; this label has no inset.
        field.frame = NSRect(x: c.minX + inset + 6, y: c.midY - fh / 2,
                             width: c.width - inset - 14 - (showChip ? chipW + 6 : 0), height: fh)
        chip.frame = NSRect(x: c.maxX - chipW - 4, y: c.midY - 9, width: chipW, height: 18)
        chip.alphaValue = showChip ? 1 : 0
        let tw = ceil(field.attributedStringValue.size().width)
        caret.frame = NSRect(x: min(field.frame.minX + tw + 1, field.frame.maxX), y: c.midY - 8, width: 1, height: 16)
    }

    /// Blinks the caret (the demo's clock drives it).
    func setCaretVisible(_ on: Bool) { caret.alphaValue = on ? 1 : 0 }
}

/// The suggestion list as PopupPanel shows it: popover material, 8 pt
/// corners, a hairline border, a shadow, and PopupListView's own rows.
@MainActor
final class DemoPopupView: NSView {
    private let fx = NSVisualEffectView()
    private let list = PopupListView()
    private var spec: DemoPopup?

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.masksToBounds = false
        shadow = {
            let s = NSShadow()
            s.shadowColor = NSColor.black.withAlphaComponent(0.3)
            s.shadowBlurRadius = 12
            s.shadowOffset = NSSize(width: 0, height: -4)
            return s
        }()
        fx.material = .popover
        fx.state = .active
        fx.blendingMode = .withinWindow
        fx.wantsLayer = true
        fx.layer?.cornerRadius = 8
        fx.layer?.masksToBounds = true
        fx.layer?.borderWidth = 0.5
        list.rowHeight = PathBarStyle.suggestionRowHeight
        addSubview(fx)
        fx.addSubview(list)
    }
    required init?(coder: NSCoder) { fatalError() }

    override var isFlipped: Bool { true }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    var hasRows: Bool { !(spec?.rows.isEmpty ?? true) }
    var contentHeight: CGFloat { list.contentHeight }

    func apply(_ p: DemoPopup?) {
        guard p != spec else { return }
        spec = p
        list.rows = p?.rows ?? []
        list.highlight = p?.highlight
        list.needsDisplay = true
    }

    override func layout() {
        super.layout()
        fx.frame = bounds
        list.frame = NSRect(x: 0, y: 0, width: bounds.width, height: list.contentHeight)
        effectiveAppearance.performAsCurrentDrawingAppearance {
            self.fx.layer?.borderColor = NSColor.separatorColor.cgColor
        }
    }
}

/// A file list: the app's table look (plain style, 22 pt rows, 8 pt column
/// spacing, the 28 pt FileHeaderView, Name sorted ascending) drawing the
/// app's FileRowView and FileCellView.
@MainActor
final class DemoListView: NSView, NSTableViewDataSource, NSTableViewDelegate {
    private let scroll = NSScrollView()
    private let table = NSTableView()
    private var rows: [Row] = []
    private var active = true
    private var columnIDs: [ColumnID] = []
    private var fixedWidths: [CGFloat] = []

    override init(frame: NSRect) {
        super.init(frame: frame)
        table.style = .plain
        table.usesAlternatingRowBackgroundColors = false
        table.backgroundColor = .textBackgroundColor
        table.gridStyleMask = []
        table.intercellSpacing = NSSize(width: 8, height: 0)
        table.rowHeight = 22
        table.focusRingType = .none
        table.columnAutoresizingStyle = .noColumnAutoresizing
        table.headerView = FileHeaderView(frame: .zero)
        table.dataSource = self
        table.delegate = self
        scroll.documentView = table
        scroll.hasVerticalScroller = false
        scroll.hasHorizontalScroller = false
        scroll.drawsBackground = true
        scroll.backgroundColor = .textBackgroundColor
        addSubview(scroll)
    }
    required init?(coder: NSCoder) { fatalError() }

    override var isFlipped: Bool { true }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    func update(rows: [Row], active: Bool, columns: [(ColumnID, CGFloat)]) {
        let ids = columns.map(\.0), widths = columns.map(\.1)
        if ids != columnIDs || widths != fixedWidths {
            columnIDs = ids
            fixedWidths = widths
            for c in table.tableColumns { table.removeTableColumn(c) }
            for id in ids {
                let c = NSTableColumn(identifier: NSUserInterfaceItemIdentifier(id.rawValue))
                c.title = id.title
                c.headerCell.alignment = id == .size ? .right : .left
                table.addTableColumn(c)
                if id == .name {
                    table.setIndicatorImage(NSImage(named: "NSAscendingSortIndicator"), in: c)
                    table.highlightedTableColumn = c
                }
            }
            needsLayout = true
        }
        if rows != self.rows || active != self.active {
            self.rows = rows
            self.active = active
            table.reloadData()
        }
    }

    override func layout() {
        super.layout()
        scroll.frame = bounds
        // Fixed widths for the secondary columns; Name takes the rest (0 = fill).
        let spacing = table.intercellSpacing.width * CGFloat(max(columnIDs.count, 1))
        let fixed = fixedWidths.reduce(0, +)
        for (c, w) in zip(table.tableColumns, fixedWidths) {
            c.width = w > 0 ? w : max(bounds.width - fixed - spacing, 80)
        }
        table.sizeLastColumnToFit()
    }

    func numberOfRows(in tableView: NSTableView) -> Int { rows.count }

    func tableView(_ tv: NSTableView, rowViewForRow row: Int) -> NSTableRowView? {
        let rv = FileRowView()
        let r = rows[row]
        rv.coreSelected = r.isSelected
        rv.isCursorRow = r.isCursor
        rv.paneActive = active
        return rv
    }

    /// As FileTable.configure: name with icon, sizes right-aligned, the rest secondary.
    func tableView(_ tv: NSTableView, viewFor col: NSTableColumn?, row i: Int) -> NSView? {
        guard let col else { return nil }
        let cell = FileCellView(frame: .zero)
        let r = rows[i]
        let onAccent = r.isSelected && active
        switch ColumnID(rawValue: col.identifier.rawValue) ?? .name {
        case .name:
            cell.kind = .name
            cell.configure(text: r.name, icon: IconCache.shared.icon(key: r.iconKey), row: r, onAccent: onAccent)
        case .size:
            cell.kind = .text(rightAligned: true)
            cell.configure(text: r.sizeText, row: r, onAccent: onAccent)
        case .modified, .created:
            cell.kind = .text(rightAligned: false)
            cell.configure(text: r.modifiedText, row: r, onAccent: onAccent)
        case .kind:
            cell.kind = .text(rightAligned: false)
            cell.configure(text: r.kindText, row: r, onAccent: onAccent)
        case .ext:
            cell.kind = .text(rightAligned: false)
            cell.configure(text: r.extText, row: r, onAccent: onAccent)
        case .rating, .label, .flag:
            cell.kind = .text(rightAligned: false)
            cell.configure(text: "", row: r, onAccent: onAccent)
        }
        return cell
    }

    func tableView(_ tableView: NSTableView, shouldSelectRow row: Int) -> Bool { false }
}

/// PaneStatusLine's look for fixed text (the real one reads a live tab).
struct DemoStatusLine: View {
    let left: String
    let message: String?
    let free: String

    var body: some View {
        HStack(spacing: 6) {
            Text(left).lineLimit(1).truncationMode(.middle)
            Spacer(minLength: 8)
            if let message { Text(message).lineLimit(1) }
            Text("\(free) available").lineLimit(1)
        }
        .font(.system(size: 11).monospacedDigit())
        .foregroundStyle(.secondary)
        .padding(.horizontal, 10)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Color(nsColor: .windowBackgroundColor))
        .overlay(alignment: .top) { Rectangle().fill(Color(nsColor: .separatorColor)).frame(height: 1) }
    }
}

/// The frame the demos' window crops sit in: the window's corner radius and edge.
struct DemoWindowFrame: ViewModifier {
    func body(content: Content) -> some View {
        content
            .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous).strokeBorder(Color.primary.opacity(0.15)))
            .shadow(color: .black.opacity(0.18), radius: 6, y: 2)
    }
}

extension View {
    func demoWindowFrame() -> some View { modifier(DemoWindowFrame()) }
}

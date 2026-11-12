// SidebarDragProbe.swift — DEBUG-only: simulates dragging a sidebar row and
// letting go over a target, through the *real* code on both ends. The
// sidebar's own `pasteboardWriterForItem` fills the pasteboard, the dragging
// source is the sidebar's outline view, and the drop goes to the target's
// own validate/accept methods: a list row (FileTable), an address-bar crumb
// (CrumbStripView's NSDraggingDestination methods) or another sidebar row
// (the outline's data source). Scripts can't start a real AppKit drag
// session, so this is the closest repeatable proof (incident 2026-11-12,
// F-023).
//
//   simdrag:<sidebar path>|row:<name in the focused list>[|legacy]
//   simdrag:<sidebar path>|crumb:<folder path in the focused pane's address bar>[|legacy]
//   simdrag:<sidebar path>|sidebar:<another sidebar row's path>[|legacy]
//   simdrag:<sidebar path>|gap:<index in Favourites>
//   moveanswer:<move|cancel>   answers the open "Move …?" confirmation (F-023)
//   protect:<folder>[,<folder>…]   adds folders to operations.protected_folders (real core)
//   fstree:<folder>            FSTREE lines: every folder and file two levels down
//
// `legacy` writes the pasteboard the sidebar wrote before the fix (its own
// type plus the folder's file URL), to reproduce the incident. Prints
// SIMDRAG lines: the operation each target offered and whether it accepted.

#if DEBUG
import AppKit

/// A stand-in for AppKit's dragging info, with a real pasteboard and source.
@MainActor
final class ProbeDraggingInfo: NSObject, @preconcurrency NSDraggingInfo {
    let pasteboard: NSPasteboard
    let source: AnyObject?
    var location: NSPoint
    weak var window: NSWindow?
    private static var seq = 9_000

    init(pasteboard: NSPasteboard, source: AnyObject?, location: NSPoint, window: NSWindow?) {
        self.pasteboard = pasteboard; self.source = source; self.location = location; self.window = window
        Self.seq += 1
        sequence = Self.seq
    }

    let sequence: Int
    var draggingDestinationWindow: NSWindow? { window }
    var draggingSourceOperationMask: NSDragOperation { [.copy, .move, .link, .generic] }
    var draggingLocation: NSPoint { location }
    var draggedImageLocation: NSPoint { location }
    var draggedImage: NSImage? { nil }
    var draggingPasteboard: NSPasteboard { pasteboard }
    var draggingSource: Any? { source }
    var draggingSequenceNumber: Int { sequence }
    func slideDraggedImage(to screenPoint: NSPoint) {}
    var draggingFormation: NSDraggingFormation = .default
    var animatesToDestination = false
    var numberOfValidItemsForDrop = 1
    func enumerateDraggingItems(options enumOpts: NSDraggingItemEnumerationOptions = [], for view: NSView?,
                                classes classArray: [AnyClass], searchOptions: [NSPasteboard.ReadingOptionKey: Any] = [:],
                                using block: (NSDraggingItem, Int, UnsafeMutablePointer<ObjCBool>) -> Void) {}
    var springLoadingHighlight: NSSpringLoadingHighlight { .none }
    func resetSpringLoading() {}
}

@MainActor
enum SidebarDragProbe {
    static func install() {
        DebugAutomation.register("simdrag") { arg, m in run(arg, m) }
        DebugAutomation.register("moveanswer") { arg, _ in
            print("MOVEANSWER \(arg) open=\(MoveSafety.openAlert != nil)")
            MoveSafety.answer(move: arg == "move")
        }
        DebugAutomation.register("protect") { arg, m in
            let current = (try? m.engine.setting("operations.protected_folders")) ?? "[]"
            var list = (try? JSONDecoder().decode([String].self, from: Data(current.utf8))) ?? []
            list += arg.split(separator: ",").map(String.init)
            do {
                try m.engine.setSetting("operations.protected_folders", json: String(data: try JSONEncoder().encode(list), encoding: .utf8)!)
                print("PROTECT now \(list.count) folders")
            } catch { print("PROTECT error \(error)") }
        }
        DebugAutomation.register("fstree") { dir, _ in
            let fm = FileManager.default
            var lines: [String] = []
            for a in ((try? fm.contentsOfDirectory(atPath: dir)) ?? []).sorted() {
                lines.append(a)
                for b in ((try? fm.contentsOfDirectory(atPath: dir + "/" + a)) ?? []).sorted() { lines.append(a + "/" + b) }
            }
            print("FSTREE " + lines.joined(separator: "  "))
        }
    }

    private static var controller: MainWindowController? {
        (NSApp.keyWindow?.windowController as? MainWindowController)
            ?? NSApp.windows.compactMap { $0.windowController as? MainWindowController }.first
    }

    private static func run(_ arg: String, _ m: AppModel) {
        let parts = arg.split(separator: "|").map(String.init)
        guard parts.count >= 2, let c = controller, let w = c.window, let sb = c.sidebar else {
            print("SIMDRAG bad arg \(arg)"); return
        }
        let legacy = parts.count > 2 && parts[2] == "legacy"
        // The pasteboard the sidebar writes for that row (or wrote before the fix).
        guard let row = (0..<sb.outline.numberOfRows).first(where: { (sb.outline.item(atRow: $0) as? SidebarNode)?.path == parts[0] }),
              let node = sb.outline.item(atRow: row) as? SidebarNode else { print("SIMDRAG no sidebar row \(parts[0])"); return }
        let pb = NSPasteboard(name: .init("fstudio-simdrag-\(UUID().uuidString)"))
        pb.clearContents()
        if legacy {
            let item = NSPasteboardItem()
            item.setString(parts[0], forType: SidebarViewController.favouriteType)
            item.setString(URL(fileURLWithPath: parts[0]).absoluteString, forType: .fileURL)
            pb.writeObjects([item])
        } else if let writer = sb.outlineView(sb.outline, pasteboardWriterForItem: node) {
            pb.writeObjects([writer])
        }
        defer { pb.releaseGlobally() }
        print("SIMDRAG from sidebar \(parts[0])\(legacy ? " (legacy pasteboard)" : "") types=\(pb.types?.map(\.rawValue) ?? [])")
        let target = parts[1]
        if target.hasPrefix("row:") {
            let name = String(target.dropFirst(4))
            let names = m.focusedTab?.api.rows(start: 0, count: 500).map(\.name) ?? []
            guard let pane = c.activePane, let ft = pane.list.table.delegate as? FileTable,
                  let i = names.firstIndex(of: name) else {
                print("SIMDRAG no list row \(name)"); return
            }
            let info = ProbeDraggingInfo(pasteboard: pb, source: sb.outline, location: .zero, window: w)
            let op = ft.tableView(pane.list.table, validateDrop: info, proposedRow: i, proposedDropOperation: .on)
            print("SIMDRAG list row “\(name)” offered=\(describe(op))")
            if !op.isEmpty {
                let ok = ft.tableView(pane.list.table, acceptDrop: info, row: i, dropOperation: .on)
                print("SIMDRAG list row “\(name)” accepted=\(ok)")
            }
        } else if target.hasPrefix("crumb:") {
            let path = String(target.dropFirst(6))
            guard let strip = findStrip(in: c.activePane?.view),
                  let i = strip.crumbs.firstIndex(where: { $0.path == path }),
                  let r = strip.rect(of: .crumb(i)) else { print("SIMDRAG no crumb \(path)"); return }
            let loc = strip.convert(NSPoint(x: r.midX, y: r.midY), to: nil)
            let info = ProbeDraggingInfo(pasteboard: pb, source: sb.outline, location: loc, window: w)
            let op = strip.draggingEntered(info)
            print("SIMDRAG crumb “\(path)” offered=\(describe(op))")
            if !op.isEmpty, strip.prepareForDragOperation(info) {
                print("SIMDRAG crumb “\(path)” accepted=\(strip.performDragOperation(info))")
            }
            strip.draggingEnded(info)
        } else if target.hasPrefix("sidebar:") || target.hasPrefix("gap:") {
            let info = ProbeDraggingInfo(pasteboard: pb, source: sb.outline, location: .zero, window: w)
            var item: Any?
            var index = NSOutlineViewDropOnItemIndex
            if target.hasPrefix("gap:") {
                item = sb.outline.item(atRow: 0)   // the Favourites section
                index = Int(target.dropFirst(4)) ?? 0
            } else {
                let p = String(target.dropFirst(8))
                item = (0..<sb.outline.numberOfRows).compactMap { sb.outline.item(atRow: $0) as? SidebarNode }.first { $0.path == p }
                if item == nil { print("SIMDRAG no sidebar row \(p)"); return }
            }
            let op = sb.outlineView(sb.outline, validateDrop: info, proposedItem: item, proposedChildIndex: index)
            print("SIMDRAG sidebar \(target) offered=\(describe(op))")
            if !op.isEmpty {
                // NSOutlineView then calls acceptDrop with the item/index validate retargeted to.
                print("SIMDRAG sidebar \(target) accepted=\(sb.debugAcceptRetargetedDrop(info))")
            }
        } else {
            print("SIMDRAG unknown target \(target)")
        }
    }

    private static func describe(_ op: NSDragOperation) -> String {
        if op.isEmpty { return "none" }
        var s: [String] = []
        if op.contains(.move) { s.append("move") }
        if op.contains(.copy) { s.append("copy") }
        if op.contains(.link) { s.append("link") }
        return s.joined(separator: "+")
    }

    private static func findStrip(in v: NSView?) -> CrumbStripView? {
        guard let v else { return nil }
        if let s = v as? CrumbStripView { return s }
        for sub in v.subviews { if let s = findStrip(in: sub) { return s } }
        return nil
    }
}
#endif

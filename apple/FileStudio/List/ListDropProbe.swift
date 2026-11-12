// ListDropProbe.swift — DEBUG-only steps that drive the list's real
// validateDrop / acceptDrop / draggingExited with a stand-in dragging info
// (scripts can't start an NSDraggingSession), for safety audit S8:
//
//   fdrag:validate:<src path>|<folder row name>   validates a drop of the file on that row
//   fdrag:accept                                  accepts it at the same row index
//   fdrag:exit                                    the drag leaves the table
//   fdrag:spring                                  prints whether spring-loading is armed
//   touch:<path>                                  creates an empty file (another app saving)
//   mkdir:<path>                                  creates a folder

#if DEBUG
import AppKit

@MainActor
enum ListDropProbe {
    private static var pending: (ProbeDraggingInfo, Int, FileTable, NSTableView, NSPasteboard)?
    private static var installed = false

    static func install() {
        guard !installed else { return }
        installed = true
        DebugAutomation.register("touch") { arg, _ in
            FileManager.default.createFile(atPath: arg, contents: Data())
        }
        DebugAutomation.register("mkdir") { arg, _ in
            try? FileManager.default.createDirectory(atPath: arg, withIntermediateDirectories: false)
        }
        DebugAutomation.register("fdrag") { arg, m in run(arg, m) }
    }

    private static func run(_ arg: String, _ m: AppModel) {
        guard let c = (NSApp.keyWindow?.windowController as? MainWindowController)
                ?? NSApp.windows.compactMap({ $0.windowController as? MainWindowController }).first,
              let pane = c.activePane, let ft = pane.list.table.delegate as? FileTable, let w = c.window else {
            print("FDRAG no list"); return
        }
        let table = pane.list.table
        if arg.hasPrefix("validate:") {
            let parts = arg.dropFirst(9).split(separator: "|").map(String.init)
            let names = m.focusedTab?.api.rows(start: 0, count: 500).map(\.name) ?? []
            guard parts.count == 2, let i = names.firstIndex(of: parts[1]) else { print("FDRAG bad \(arg) rows=\(names)"); return }
            let pb = NSPasteboard(name: .init("fstudio-fdrag-\(UUID().uuidString)"))
            pb.clearContents()
            pb.writeObjects([URL(fileURLWithPath: parts[0]) as NSURL])
            let info = ProbeDraggingInfo(pasteboard: pb, source: nil, location: .zero, window: w)
            let op = ft.tableView(table, validateDrop: info, proposedRow: i, proposedDropOperation: .on)
            print("FDRAG validate row \(i) “\(parts[1])” offered=\(op.rawValue)")
            pending = (info, i, ft, table, pb)
        } else if arg == "accept", let (info, i, ft, table, pb) = pending {
            print("FDRAG state \(ft.debugDropState(row: i))")
            let ok = ft.tableView(table, acceptDrop: info, row: i, dropOperation: .on)
            print("FDRAG accept row \(i) accepted=\(ok)")
            pb.releaseGlobally()
            pending = nil
        } else if arg == "exit" {
            table.draggingExited(pending?.0)
            print("FDRAG exited")
        } else if arg == "spring" {
            print("FDRAG spring armed=\(ft.springArmed)")
        }
    }
}
#endif

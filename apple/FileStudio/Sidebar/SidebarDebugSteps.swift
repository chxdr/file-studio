// SidebarDebugSteps.swift — DEBUG-only script steps for pinning and
// unpinning favourites (registered with DebugAutomation). Drags can't be
// injected from a script, so the drag steps call the same drop and drag-out
// code the sidebar's NSDraggingSource / NSOutlineView drop methods call, with
// a real pasteboard. Menu steps fire items of the real NSMenus the core's
// context menus are drawn as.
//
//   favlog                     FAV lines: the core's favourites (path, name, missing) and the sidebar rows
//   favdrop:<path>[,<index>]   drops a folder (a file URL, as from the list, Finder or another app)
//                              on Favourites at <index> (default: the end)
//   favmove:<path>,<index>     drags a favourite to the gap at <index> (reorder)
//   favdragout:<path>          drags a favourite out of the sidebar and lets go over the list (poof)
//   favselect:<path>           selects that sidebar row (then `focus:sidebar;key:delete` removes it)
//   favmenu:<path>|<title>     right-click on that sidebar row › the item titled <title>
//   foldermenu:<name>|<title>  right-click on <name> in the focused list › the item titled <title>
//   tabmenu:<title>            right-click on the focused tab › the item titled <title>
//   favrename:<text>|<ok|cancel>   types into the open Rename Favourite sheet and ends it
//   tagclick:<name>            clicks the sidebar tag <name> (searches the Mac for it)

#if DEBUG
import AppKit

@MainActor
enum SidebarDebugSteps {
    private static var installed = false

    static func install() {
        guard !installed else { return }
        installed = true
        DebugAutomation.register("favlog") { _, m in log(m) }
        DebugAutomation.register("favdrop") { arg, m in
            guard let s = sidebar else { return }
            let parts = arg.split(separator: ",", maxSplits: 1).map(String.init)
            let pb = NSPasteboard(name: .init("fstudio-debug-\(UUID().uuidString)"))
            pb.clearContents()
            pb.writeObjects([URL(fileURLWithPath: parts[0]) as NSURL])
            let at = parts.count > 1 ? Int(parts[1]) ?? m.favourites.count : m.favourites.count
            print("FAVDROP \(parts[0]) at=\(at) accepted=\(s.acceptDrop(pb, at: at))")
            pb.releaseGlobally()
        }
        DebugAutomation.register("favmove") { arg, _ in
            guard let s = sidebar else { return }
            let parts = arg.split(separator: ",").map(String.init)
            guard parts.count == 2, let at = Int(parts[1]) else { return }
            let pb = NSPasteboard(name: .init("fstudio-debug-\(UUID().uuidString)"))
            pb.clearContents()
            pb.setString(parts[0], forType: SidebarViewController.favouriteType)
            print("FAVMOVE \(parts[0]) to=\(at) accepted=\(s.acceptDrop(pb, at: at))")
            pb.releaseGlobally()
        }
        DebugAutomation.register("favdragout") { path, _ in
            guard let s = sidebar, let w = s.view.window, let content = w.contentView else { return }
            // Let go over the middle of the window (the file list), as a real drag-out would.
            let inside = w.convertPoint(toScreen: s.view.convert(NSPoint(x: s.view.bounds.midX, y: s.view.bounds.midY), to: nil))
            let over = w.convertPoint(toScreen: NSPoint(x: content.bounds.midX + 150, y: content.bounds.midY))
            print("FAVDRAGOUT \(path) overSidebar(inside)=\(s.isOverSidebar(inside)) overSidebar(letGo)=\(s.isOverSidebar(over))")
            if !s.isOverSidebar(over) { s.dragOut(path, at: over) }
        }
        DebugAutomation.register("tagclick") { name, _ in
            print("TAGCLICK \(name) \(sidebar?.debugClickTag(name) ?? false)")
        }
        DebugAutomation.register("favselect") { path, _ in
            print("FAVSELECT \(path) \(sidebar?.debugSelect(path: path) ?? false)")
        }
        DebugAutomation.register("favmenu") { arg, _ in
            let parts = arg.split(separator: "|", maxSplits: 1).map(String.init)
            guard parts.count == 2 else { return }
            fire(sidebar?.debugMenu(path: parts[0]), title: parts[1], what: "sidebar \(parts[0])")
        }
        DebugAutomation.register("foldermenu") { arg, m in
            let parts = arg.split(separator: "|", maxSplits: 1).map(String.init)
            guard parts.count == 2, let t = m.focusedTab else { return }
            let path = (t.location as NSString).appendingPathComponent(parts[0])
            let items = (try? m.engine.contextMenu(.folder(tab: t.id, path: path), context: MenuContext())) ?? []
            fire(ContextMenuBuilder.menu(items, model: m), title: parts[1], what: "folder \(parts[0])")
        }
        DebugAutomation.register("tabmenu") { title, m in
            guard let t = m.focusedTab else { return }
            let items = (try? m.engine.contextMenu(.tab(t.id), context: MenuContext())) ?? []
            fire(ContextMenuBuilder.menu(items, model: m), title: title, what: "tab \(t.location)")
        }
        DebugAutomation.register("favrename") { arg, _ in
            let parts = arg.split(separator: "|", maxSplits: 1, omittingEmptySubsequences: false).map(String.init)
            guard let f = SidebarServices.renameField else { print("FAVRENAME no sheet"); return }
            f.stringValue = parts[0]
            print("FAVRENAME \(parts[0])")
            SidebarServices.endRename(ok: parts.count < 2 || parts[1] != "cancel")
        }
    }

    private static var sidebar: SidebarViewController? {
        ((NSApp.keyWindow?.windowController as? MainWindowController)
            ?? NSApp.windows.compactMap { $0.windowController as? MainWindowController }.first)?.sidebar
    }

    /// Prints a menu (MENUITEM lines) and fires the item with that title, as a click would.
    private static func fire(_ menu: NSMenu?, title: String, what: String) {
        guard let menu else { print("FAVMENU no menu for \(what)"); return }
        print("FAVMENU \(what): " + menu.items.map { $0.isSeparatorItem ? "---" : $0.title }.joined(separator: " · "))
        guard let i = menu.items.firstIndex(where: { $0.title == title }) else {
            print("FAVMENU no item “\(title)”")
            return
        }
        menu.performActionForItem(at: i)
        print("FAVMENU fired “\(title)”")
    }

    private static func log(_ m: AppModel) {
        for f in m.engine.favourites() {
            print("FAV \(f.name) | \(f.path)\(f.renamed ? " | renamed" : "")\(f.missing ? " | missing" : "")")
        }
        for r in sidebar?.debugRows ?? [] { print("FAV SIDEBAR \(r)") }
        if let s = m.statusMessage { print("FAV STATUS \(s)") }
    }
}
#endif

// NetworkDebugSteps.swift — DEBUG-only script steps for Connect to Server
// and the sidebar's Network section (registered with DebugAutomation; run
// them with scripts/run-debug.sh). Steps drive the real sheet and rows.
//
//   netlog                    NET lines: the Network section rows, recent servers, the sheet's state
//   netsheet                  opens the Connect to Server sheet (same as key:cmd+k)
//   nettype:<text>            replaces the sheet's address with <text> (checked as typed)
//   netsidebar:<on|off>       ticks or unticks the sheet's "Add to Sidebar"
//   netconnect                presses Connect in the sheet
//   netcancel                 presses Cancel in the sheet
//   netsave:<address>         saves a server to the sidebar
//   netforget:<address>       removes a saved server
//   netclick:<url>            clicks the server row with that address (connects)
//   netmenu:<url>|<title>     right-click on that server row › the item titled <title>
//   netclear                  clears the recent servers

#if DEBUG
import AppKit

@MainActor
enum NetworkDebugSteps {
    private static var installed = false

    static func install() {
        guard !installed else { return }
        installed = true
        DebugAutomation.register("netlog") { _, m in log(m) }
        DebugAutomation.register("netsheet") { _, m in
            ConnectServerSheet.show(model: m, window: m.keyWindowID, tab: m.focusedTab?.id)
            print("NETSHEET open=\(ConnectServerSheet.current != nil)")
        }
        DebugAutomation.register("nettype") { text, _ in
            guard let s = ConnectServerSheet.current else { print("NETTYPE no sheet"); return }
            s.field.stringValue = text
            s.controlTextDidChange(Notification(name: NSControl.textDidChangeNotification, object: s.field))
            print("NETTYPE \(text) → \(s.debugStatus)")
        }
        DebugAutomation.register("netsidebar") { arg, _ in
            ConnectServerSheet.current?.addToSidebar.state = arg == "off" ? .off : .on
        }
        DebugAutomation.register("netconnect") { _, m in
            guard let s = ConnectServerSheet.current else { print("NETCONNECT no sheet"); return }
            s.connect()
            print("NETCONNECT sheetOpen=\(ConnectServerSheet.current != nil) status=\(m.statusMessage ?? "")")
        }
        DebugAutomation.register("netcancel") { _, _ in ConnectServerSheet.current?.cancel() }
        DebugAutomation.register("netsave") { addr, m in
            do { try m.network?.saveServer(addr); print("NETSAVE \(addr)") } catch { print("NETSAVE error \(error.localizedDescription)") }
            sidebar?.reload()
        }
        DebugAutomation.register("netforget") { addr, m in
            do { try m.network?.removeSavedServer(addr); print("NETFORGET \(addr)") } catch {
                print("NETFORGET error \(error.localizedDescription)")
            }
            sidebar?.reload()
        }
        DebugAutomation.register("netclick") { url, m in
            print("NETCLICK \(url) \(sidebar?.debugClickServer(url) ?? false) status=\(m.statusMessage ?? "")")
        }
        DebugAutomation.register("netmenu") { arg, _ in
            let parts = arg.split(separator: "|", maxSplits: 1).map(String.init)
            guard parts.count == 2, let menu = sidebar?.debugServerMenu(parts[0]) else { print("NETMENU no menu"); return }
            print("NETMENU \(parts[0]): " + menu.items.map { $0.isSeparatorItem ? "---" : $0.title }.joined(separator: " · "))
            if let i = menu.items.firstIndex(where: { $0.title == parts[1] }) {
                menu.performActionForItem(at: i)
                print("NETMENU fired “\(parts[1])”")
            }
        }
        DebugAutomation.register("netclear") { _, m in try? m.network?.clearRecentServers() }
    }

    private static var sidebar: SidebarViewController? {
        ((NSApp.keyWindow?.windowController as? MainWindowController)
            ?? NSApp.windows.compactMap { $0.windowController as? MainWindowController }.first)?.sidebar
    }

    private static func log(_ m: AppModel) {
        let rows = sidebar?.debugRows ?? []
        var inNetwork = false
        for r in rows {
            if r.hasPrefix("# ") { inNetwork = r == "# Network" }
            if inNetwork { print("NET SIDEBAR \(r)") }
        }
        if !rows.contains("# Network") { print("NET SIDEBAR (no Network section)") }
        for s in m.network?.recentServers() ?? [] { print("NET RECENT \(s.name) | \(s.url)") }
        if let p = m.network?.networkSection().browseProblem { print("NET PROBLEM \(p)") }
        if let s = ConnectServerSheet.current {
            print("NET SHEET field=\(s.field.stringValue) status=\(s.debugStatus)")
            for r in s.debugRows { print("NET SHEET RECENT \(r)") }
        }
        if let s = m.statusMessage { print("NET STATUS \(s)") }
    }
}
#endif

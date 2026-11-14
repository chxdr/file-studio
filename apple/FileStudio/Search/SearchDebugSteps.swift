// SearchDebugSteps.swift — DEBUG-only script steps for search (registered
// with DebugAutomation the first time a search bar is made):
//
//   searchset:<key>=<value>   sets a search bar control on the active pane and searches:
//                             text, scope (folder|mac), mode (contains|glob|regex), kind
//                             (a menu title), size, date, content, tags, filters (on|off)
//   searchwait[:<s>]          waits until the active tab's search is no longer running (max 30 s)
//   searchlog                 SEARCH line (bar + status) and SEARCHROW lines for the first 12 rows

#if DEBUG
import AppKit

@MainActor
enum SearchDebugSteps {
    private static var installed = false

    static func install() {
        guard !installed else { return }
        installed = true
        DebugAutomation.register("searchset") { arg, _ in
            guard let bar = pane?.searchBar else { return }
            let kv = arg.split(separator: "=", maxSplits: 1).map(String.init)
            bar.debugSet(kv[0], kv.count > 1 ? kv[1] : "")
        }
        DebugAutomation.register("searchlog") { _, m in log(m) }
        DebugAutomation.extraSteps["searchwait"] = { arg in
            let limit = Date().addingTimeInterval(Double(arg) ?? 30)
            while Date() < limit {
                let st = (pane?.searchBar.tab?.api as? SearchTabAPI)?.searchStatus()
                if st == nil || st?.state != .searching { break }
                try? await Task.sleep(for: .milliseconds(50))
            }
            pane?.searchBar.refresh()
        }
    }

    private static var pane: PaneViewController? {
        ((NSApp.keyWindow?.windowController as? MainWindowController)
            ?? NSApp.windows.compactMap { $0.windowController as? MainWindowController }.first)?.activePane
    }

    private static func log(_ m: AppModel) {
        guard let p = pane, let tm = p.searchBar.tab else { print("SEARCH none"); return }
        let st = (tm.api as? SearchTabAPI)?.searchStatus()
        var d = p.searchBar.debugInfo
        d["state"] = st.map { "\($0.state)" } ?? "folder"
        d["found"] = st?.found ?? 0
        d["firstHitMs"] = st?.firstHitMs.map { Int($0) } ?? -1
        d["elapsedMs"] = st?.elapsedMs ?? 0
        d["denied"] = st?.deniedCount ?? 0
        d["indexUsed"] = st?.indexUsed ?? false
        d["location"] = tm.location
        d["rows"] = tm.listing.count
        let json = (try? JSONSerialization.data(withJSONObject: d, options: [.sortedKeys]))
            .flatMap { String(data: $0, encoding: .utf8) } ?? "\(d)"
        print("SEARCH \(json)")
        for r in tm.api.rows(start: 0, count: 12) {
            print("SEARCHROW \(r.index) \(r.isSelected ? "[x]" : "[ ]") \(r.shownName) | \(r.locationText) | \(r.kindText) | id=\(r.name)")
        }
    }
}
#endif

// NetworkRust.swift — NetworkEngineAPI over the UniFFI bindings. Pure translation.

import Foundation

private func netWrap<T>(_ body: () throws -> T) throws -> T {
    do { return try body() } catch let e as FfiError {
        switch e {
        case .NotFound(let m): throw CoreError(.notFound, m)
        case .NotAvailable(let m): throw CoreError(.notAvailable, m)
        case .InvalidName(let m): throw CoreError(.invalidName, m)
        case .InvalidSetting(let m): throw CoreError(.invalidSetting, m)
        case .UnknownCommand(let m): throw CoreError(.unknownCommand, m)
        case .Failed(let m): throw CoreError(.failed, m)
        }
    }
}

extension NetworkServer {
    init(_ s: FfiServer) {
        let kind: NetworkServerKind = switch s.kind {
        case .nearby: .nearby
        case .saved: .saved
        case .recent: .recent
        }
        self.init(url: s.url, name: s.name, kind: kind, connecting: s.connecting, mountedAt: s.mountedAt,
                  saved: s.saved)
    }
}

extension RustEngine: NetworkEngineAPI {
    func connectServer(_ address: String, tab: TabID?) throws -> Outcome {
        try netWrap { Outcome(try ffi.connectServer(address: address, tab: tab)) }
    }
    func cancelConnect(_ address: String) throws -> Bool {
        try netWrap { try ffi.cancelConnect(address: address) }
    }
    func checkServerAddress(_ address: String) throws -> String {
        try netWrap { try ffi.checkServerAddress(address: address) }
    }
    func recentServers() -> [NetworkServer] { ffi.recentServers().map(NetworkServer.init) }
    func clearRecentServers() throws { try netWrap { try ffi.clearRecentServers() } }
    func saveServer(_ address: String) throws { try netWrap { try ffi.saveServer(address: address) } }
    func removeSavedServer(_ address: String) throws { try netWrap { try ffi.removeSavedServer(address: address) } }
    func startNetworkBrowsing() { ffi.startNetworkBrowsing() }
    func networkSection() -> NetworkSection {
        let n = ffi.networkSidebar()
        return NetworkSection(computers: n.computers.map(NetworkServer.init), saved: n.saved.map(NetworkServer.init),
                              shares: n.shares.map(Volume.init), browseProblem: n.browseProblem)
    }
}

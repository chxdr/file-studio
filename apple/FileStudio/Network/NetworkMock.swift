// NetworkMock.swift — NetworkEngineAPI for MockCore runs (no Rust). A
// rough stand-in so the sheet and sidebar section can be drawn without the
// core; nothing is mounted. The real rules are in `fstudio-core/src/network.rs`.

import Foundation

/// The mock's network state (MockCore has one engine per run).
@MainActor
private enum MockNetwork {
    static var recent: [NetworkServer] = [
        NetworkServer(url: "smb://archbox/media", name: "media on archbox", kind: .recent),
    ]
    static var saved: [NetworkServer] = []
}

extension MockEngine: NetworkEngineAPI {
    private func canonical(_ address: String) throws -> (url: String, name: String) {
        var t = address.trimmingCharacters(in: .whitespaces)
        if t.isEmpty { throw CoreError(.invalidName, "Type a server address, like smb://archbox/media") }
        if let r = t.range(of: "://") {
            let scheme = t[..<r.lowerBound].lowercased()
            guard scheme == "smb" || scheme == "cifs" else {
                throw CoreError(.invalidName, "\(scheme):// servers aren’t supported yet; only SMB (smb://) is")
            }
            t = String(t[r.upperBound...])
        }
        let parts = t.split(separator: "/").map(String.init)
        guard let host = parts.first else { throw CoreError(.invalidName, "The address has no server name") }
        let url = "smb://" + parts.joined(separator: "/")
        return (url, parts.count > 1 ? "\(parts[1]) on \(host)" : host)
    }

    func connectServer(_ address: String, tab: TabID?) throws -> Outcome {
        let c = try canonical(address)
        MainActor.assumeIsolated {
            MockNetwork.recent.removeAll { $0.url == c.url }
            MockNetwork.recent.insert(NetworkServer(url: c.url, name: c.name, kind: .recent), at: 0)
        }
        throw CoreError(.notAvailable, "The mock core can’t mount “\(c.name)”")
    }
    func cancelConnect(_ address: String) throws -> Bool { false }
    func checkServerAddress(_ address: String) throws -> String { try canonical(address).url }
    func recentServers() -> [NetworkServer] { MainActor.assumeIsolated { MockNetwork.recent } }
    func clearRecentServers() throws { MainActor.assumeIsolated { MockNetwork.recent = [] } }
    func saveServer(_ address: String) throws {
        let c = try canonical(address)
        MainActor.assumeIsolated {
            if !MockNetwork.saved.contains(where: { $0.url == c.url }) {
                MockNetwork.saved.append(NetworkServer(url: c.url, name: c.name, kind: .saved, saved: true))
            }
        }
    }
    func removeSavedServer(_ address: String) throws {
        let c = try canonical(address)
        MainActor.assumeIsolated { MockNetwork.saved.removeAll { $0.url == c.url } }
    }
    func startNetworkBrowsing() {}
    func networkSection() -> NetworkSection {
        MainActor.assumeIsolated {
            NetworkSection(computers: [NetworkServer(url: "smb://Arch%20Box._smb._tcp.local", name: "Arch Box",
                                                     kind: .nearby)],
                           saved: MockNetwork.saved,
                           shares: volumes().filter(\.isNetwork))
        }
    }
}

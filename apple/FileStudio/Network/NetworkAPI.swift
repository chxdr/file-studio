// NetworkAPI.swift — Connect to Server (SMB) and the sidebar's Network
// section: the shell's contract with the core.
//
// The core parses addresses, keeps saved and recent servers (URLs only,
// never passwords), finds nearby SMB computers with Bonjour, and mounts
// shares with macOS's NetFS, which shows the system's own sign-in window and
// share picker. When a mount finishes, the core sends the tab to the share.
// The shell only draws the sheet and the rows (F-001).

import AppKit

/// Where a server row comes from.
public enum NetworkServerKind: Equatable, Sendable {
    case nearby, saved, recent
}

/// A server or share address.
public struct NetworkServer: Equatable, Sendable, Identifiable {
    /// The canonical address (`smb://archbox/media`), never with a password.
    public var url: String
    /// The name to show ("media on archbox", "Arch Box").
    public var name: String
    public var kind: NetworkServerKind
    /// A connection to it is in progress.
    public var connecting: Bool
    /// Where the share is mounted, when it is.
    public var mountedAt: String?
    /// Also a saved server.
    public var saved: Bool
    public var id: String { url }
    public init(url: String, name: String, kind: NetworkServerKind, connecting: Bool = false,
                mountedAt: String? = nil, saved: Bool = false) {
        self.url = url; self.name = name; self.kind = kind; self.connecting = connecting
        self.mountedAt = mountedAt; self.saved = saved
    }
}

/// Everything the sidebar's Network section shows.
public struct NetworkSection: Equatable, Sendable {
    /// Nearby SMB computers (Bonjour).
    public var computers: [NetworkServer] = []
    /// Saved servers.
    public var saved: [NetworkServer] = []
    /// Mounted network shares, each ejectable.
    public var shares: [Volume] = []
    /// Why nearby computers can't be listed (local network access refused).
    public var browseProblem: String?
    public init(computers: [NetworkServer] = [], saved: [NetworkServer] = [], shares: [Volume] = [],
                browseProblem: String? = nil) {
        self.computers = computers; self.saved = saved; self.shares = shares; self.browseProblem = browseProblem
    }
}

/// The network calls on the engine. `RustEngine` and `MockEngine` adopt it.
public protocol NetworkEngineAPI: AnyObject, Sendable {
    /// Connects to an address and opens the share in `tab` once mounted.
    /// Returns at once ("Connecting to …"); failures arrive as a toast.
    func connectServer(_ address: String, tab: TabID?) throws -> Outcome
    /// Stops a connection in progress; false when there was none.
    func cancelConnect(_ address: String) throws -> Bool
    /// The canonical form of a typed address, or why it can't be used.
    func checkServerAddress(_ address: String) throws -> String
    /// Servers connected to recently, newest first.
    func recentServers() -> [NetworkServer]
    func clearRecentServers() throws
    /// Saves an address in the sidebar's Network section.
    func saveServer(_ address: String) throws
    func removeSavedServer(_ address: String) throws
    /// Starts looking for nearby SMB computers (once; macOS may ask for
    /// local network access).
    func startNetworkBrowsing()
    func networkSection() -> NetworkSection
}

extension AppModel {
    /// The engine's network calls (both engines have them).
    var network: NetworkEngineAPI? { engine as? NetworkEngineAPI }
}

/// A menu item that runs a closure (the server rows' small menu).
@MainActor
final class ClosureMenuItem: NSMenuItem {
    private let run: () -> Void
    init(_ title: String, _ run: @escaping () -> Void) {
        self.run = run
        super.init(title: title, action: #selector(fire), keyEquivalent: "")
        target = self
    }
    required init(coder: NSCoder) { fatalError() }
    @objc private func fire() { run() }
}

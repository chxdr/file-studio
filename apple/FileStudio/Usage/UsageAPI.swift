// UsageAPI.swift — the disk usage view's contract with the core (M3).
//
// Like CoreAPI.swift, but kept in its own file so the usage view is one
// folder: `UsageEngineAPI` is implemented by RustEngine (RustCore.swift, over
// FfiUsageView) and MockEngine (UsageMock.swift). The core owns the scan, the
// tree list's rows and expansion, keyboard moves, the treemap layout, the type
// breakdown and every display string; this side only draws and forwards input.

import CoreGraphics
import Foundation

public enum UsageSizeMode: Equatable, Sendable { case onDisk, logical }

public enum UsageState: Equatable, Sendable {
    case scanning, done, cancelled
    case failed(String)
}

public enum UsageChange: Equatable, Sendable { case started, finished, treeChanged }

/// A key in the tree list or treemap, as an intent; the core decides where it goes.
public enum UsageMove: Equatable, Sendable {
    case up, down, home, end, left, right
    case pageUp(UInt32), pageDown(UInt32)
}

public struct UsageProgress: Equatable, Sendable {
    public var items: UInt64
    public var folders: UInt64
    public var allocated: UInt64
    public var logical: UInt64
    public var current: String
    public var elapsedMs: UInt64
}

/// One tree-list row, formatted by the core.
public struct UsageRow: Equatable, Sendable {
    public var id: UInt32
    public var depth: UInt16
    public var name: String
    public var sizeText: String
    public var size: UInt64
    /// Share of the parent, 0…1.
    public var fraction: Double
    public var percentText: String
    public var itemsText: String
    public var modifiedText: String
    public var kind: RowKind
    public var ext: String
    public var iconKey: String
    public var expandable: Bool
    public var expanded: Bool
    public var selected: Bool
    public var inCloud: Bool
    public var denied: Bool
    public var otherVolume: Bool
    public var note: String
}

public struct UsageNodeInfo: Equatable, Sendable {
    public var id: UInt32
    public var path: String
    public var name: String
    public var kind: RowKind
    public var allocatedText: String
    public var logicalText: String
    public var sizeText: String
    public var shareText: String
    public var itemsText: String
    public var modifiedText: String
    public var typeText: String
    public var isFolder: Bool
    public var isRoot: Bool
    public var note: String
}

public struct UsageTypeRow: Equatable, Sendable {
    public var name: String
    public var ext: String
    public var kindText: String
    public var sizeText: String
    public var size: UInt64
    public var countText: String
    public var fraction: Double
    /// Treemap colour slot: 1…12 for the top types, 0 for the rest.
    public var slot: UInt8
}

public struct UsageSummary: Equatable, Sendable {
    public var title: String
    public var path: String
    public var state: UsageState
    public var sizeText: String
    public var itemsText: String
    public var mode: UsageSizeMode
    public var detail: String
    public var progressText: String
    public var current: String
}

/// One treemap rectangle in view points (origin top-left).
public struct UsageTile: Equatable, Sendable {
    public var id: UInt32
    public var rect: CGRect
    public var depth: UInt16
    /// False for a folder frame drawn behind its children.
    public var leaf: Bool
    /// 1…12 top types, 0 other, 250 iCloud, 251 small folder, 255 frame.
    public var color: UInt8
}

/// A breadcrumb (`id` is a node) or legend entry (`id` is a colour slot).
public struct UsageLabel: Equatable, Sendable {
    public var id: UInt32
    public var name: String
}

/// One scan and its view state (`fstudio_core::UsageView`).
public protocol UsageViewAPI: AnyObject, Sendable {
    func rootPath() -> String
    func summary() -> UsageSummary
    func cancel()
    func rescan()
    /// Rescans one folder in the background.
    func refresh(_ id: UInt32)
    func setMode(_ mode: UsageSizeMode)
    func rowCount() -> UInt32
    func rows(start: UInt32, count: UInt32) -> [UsageRow]
    func setExpanded(_ id: UInt32, _ open: Bool)
    func rowOf(_ id: UInt32) -> UInt32?
    func selected() -> UInt32?
    /// Selects a node, expanding the rows above it; returns its row.
    @discardableResult func select(_ id: UInt32?) -> UInt32?
    @discardableResult func moveSelection(_ mv: UsageMove) -> UInt32?
    func info(_ id: UInt32) -> UsageNodeInfo?
    func find(_ path: String) -> UInt32?
    func paths(_ ids: [UInt32]) -> [String]
    func zoom() -> UInt32
    func setZoom(_ id: UInt32)
    @discardableResult func zoomOut() -> Bool
    func zoomPath() -> [UsageLabel]
    /// The treemap for a view of this size (cached by the core).
    func layout(width: Double, height: Double) -> [UsageTile]
    func hit(x: Double, y: Double) -> UInt32?
    func types() -> [UsageTypeRow]
    func legend() -> [UsageLabel]
    /// After a trash, undo or move. Touches the disk (`lstat` per path).
    func syncPaths(_ paths: [String])
}

/// What the engine offers the usage view.
public protocol UsageEngineAPI: AnyObject {
    /// Starts scanning; callbacks arrive on a core thread.
    func usageScan(path: String,
                   progress: @escaping @Sendable (UsageProgress) -> Void,
                   changed: @escaping @Sendable (UsageChange) -> Void) throws -> UsageViewAPI
    /// Move to Trash through the ops queue (asks first per setting; undoable).
    func usageTrash(paths: [String]) throws -> Outcome
}

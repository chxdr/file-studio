// UsageRust.swift — UsageViewAPI over the UniFFI `FfiUsageView`. Pure
// translation; no decisions. (RustEngine's `usageScan` is at the end of
// RustCore.swift, which owns the private FfiEngine.)

import CoreGraphics
import Foundation

/// Forwards the core's scan callbacks to closures.
final class UsageObserverBridge: FfiUsageObserver, @unchecked Sendable {
    private let onProgress: @Sendable (UsageProgress) -> Void
    private let onChange: @Sendable (UsageChange) -> Void

    init(progress: @escaping @Sendable (UsageProgress) -> Void,
         changed: @escaping @Sendable (UsageChange) -> Void) {
        onProgress = progress
        onChange = changed
    }

    func progress(progress p: FfiUsageProgress) {
        onProgress(UsageProgress(items: p.items, folders: p.folders, allocated: p.allocated, logical: p.logical,
                                 current: p.current, elapsedMs: p.elapsedMs))
    }

    func changed(change: FfiUsageChange) {
        let c: UsageChange = switch change {
        case .started: .started
        case .finished: .finished
        case .treeChanged: .treeChanged
        }
        onChange(c)
    }
}

private extension RowKind {
    init(usage k: FfiRowKind) {
        self = switch k {
        case .folder: .folder
        case .package: .package
        case .file: .file
        case .symlink: .symlink
        case .other: .other
        }
    }
}

final class RustUsageView: UsageViewAPI, @unchecked Sendable {
    let ffi: FfiUsageView
    init(_ ffi: FfiUsageView) { self.ffi = ffi }

    func rootPath() -> String { ffi.rootPath() }

    func summary() -> UsageSummary {
        let s = ffi.summary()
        let state: UsageState = switch s.state {
        case .scanning: .scanning
        case .done: .done
        case .cancelled: .cancelled
        case .failed(let m): .failed(m)
        }
        return UsageSummary(title: s.title, path: s.path, state: state, sizeText: s.sizeText, itemsText: s.itemsText,
                            mode: s.mode == .onDisk ? .onDisk : .logical, detail: s.detail,
                            progressText: s.progressText, current: s.current)
    }

    func cancel() { ffi.cancel() }
    func rescan() { ffi.rescan() }
    func refresh(_ id: UInt32) { ffi.refresh(id: id) }
    func setMode(_ mode: UsageSizeMode) { ffi.setMode(mode: mode == .onDisk ? .onDisk : .logical) }
    func rowCount() -> UInt32 { ffi.rowCount() }

    func rows(start: UInt32, count: UInt32) -> [UsageRow] {
        ffi.rows(start: start, count: count).map { r in
            UsageRow(id: r.id, depth: r.depth, name: r.name, sizeText: r.sizeText, size: r.size,
                     fraction: r.fraction, percentText: r.percentText, itemsText: r.itemsText,
                     modifiedText: r.modifiedText, kind: RowKind(usage: r.kind), ext: r.ext, iconKey: r.iconKey,
                     expandable: r.expandable, expanded: r.expanded, selected: r.selected, inCloud: r.inCloud,
                     denied: r.denied, otherVolume: r.otherVolume, note: r.note)
        }
    }

    func setExpanded(_ id: UInt32, _ open: Bool) { ffi.setExpanded(id: id, open: open) }
    func rowOf(_ id: UInt32) -> UInt32? { ffi.rowOf(id: id) }
    func selected() -> UInt32? { ffi.selected() }
    func select(_ id: UInt32?) -> UInt32? { ffi.select(id: id) }

    func moveSelection(_ mv: UsageMove) -> UInt32? {
        let m: FfiUsageMove = switch mv {
        case .up: .up
        case .down: .down
        case .home: .home
        case .end: .end
        case .left: .left
        case .right: .right
        case .pageUp(let n): .pageUp(rows: n)
        case .pageDown(let n): .pageDown(rows: n)
        }
        return ffi.moveSelection(mv: m)
    }

    func info(_ id: UInt32) -> UsageNodeInfo? {
        ffi.info(id: id).map { i in
            UsageNodeInfo(id: i.id, path: i.path, name: i.name, kind: RowKind(usage: i.kind),
                          allocatedText: i.allocatedText, logicalText: i.logicalText, sizeText: i.sizeText,
                          shareText: i.shareText, itemsText: i.itemsText, modifiedText: i.modifiedText,
                          typeText: i.typeText, isFolder: i.isFolder, isRoot: i.isRoot, note: i.note)
        }
    }

    func find(_ path: String) -> UInt32? { ffi.find(path: path) }
    func paths(_ ids: [UInt32]) -> [String] { ffi.paths(ids: ids) }
    func zoom() -> UInt32 { ffi.zoom() }
    func setZoom(_ id: UInt32) { ffi.setZoom(id: id) }
    func zoomOut() -> Bool { ffi.zoomOut() }
    func zoomPath() -> [UsageLabel] { ffi.zoomPath().map { UsageLabel(id: $0.id, name: $0.name) } }

    func layout(width: Double, height: Double) -> [UsageTile] {
        ffi.layout(width: width, height: height).map {
            UsageTile(id: $0.id, rect: CGRect(x: $0.x, y: $0.y, width: $0.w, height: $0.h), depth: $0.depth,
                      leaf: $0.leaf, color: $0.color)
        }
    }

    func hit(x: Double, y: Double) -> UInt32? { ffi.hit(x: x, y: y) }

    func types() -> [UsageTypeRow] {
        ffi.types().map {
            UsageTypeRow(name: $0.name, ext: $0.ext, kindText: $0.kindText, sizeText: $0.sizeText, size: $0.size,
                         countText: $0.countText, fraction: $0.fraction, slot: $0.slot)
        }
    }

    func legend() -> [UsageLabel] { ffi.legend().map { UsageLabel(id: $0.id, name: $0.name) } }
    func syncPaths(_ paths: [String]) { ffi.syncPaths(paths: paths) }
}

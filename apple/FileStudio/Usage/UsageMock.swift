// UsageMock.swift — a fixed, tiny usage tree for `-FSMockCore` runs and UI
// tests without Rust. It only fakes the contract's shape: one root with three
// children, slices for a treemap and a short type list.

import CoreGraphics
import Foundation

final class MockUsageView: UsageViewAPI, @unchecked Sendable {
    private struct Item { let id: UInt32; let name: String; let size: UInt64; let ext: String; let kind: RowKind }
    private let root: String
    private let items: [Item] = [
        Item(id: 1, name: "Photos", size: 600_000_000, ext: "", kind: .folder),
        Item(id: 2, name: "clip.mov", size: 300_000_000, ext: "mov", kind: .file),
        Item(id: 3, name: "notes.txt", size: 100_000_000, ext: "txt", kind: .file),
    ]
    private var sel: UInt32?
    private var zoomed: UInt32 = 0

    init(root: String) { self.root = root }

    private var total: UInt64 { items.reduce(0) { $0 + $1.size } }
    private func text(_ b: UInt64) -> String { ByteCountFormatter.string(fromByteCount: Int64(b), countStyle: .file) }

    func rootPath() -> String { root }
    func summary() -> UsageSummary {
        UsageSummary(title: (root as NSString).lastPathComponent, path: root, state: .done, sizeText: text(total),
                     itemsText: "3 items", mode: .onDisk, detail: "Mock data", progressText: "", current: "")
    }
    func cancel() {}
    func rescan() {}
    func refresh(_ id: UInt32) {}
    func setMode(_ mode: UsageSizeMode) {}
    func rowCount() -> UInt32 { UInt32(items.count + 1) }

    func rows(start: UInt32, count: UInt32) -> [UsageRow] {
        let all = [UsageRow(id: 0, depth: 0, name: (root as NSString).lastPathComponent, sizeText: text(total),
                            size: total, fraction: 1, percentText: "100.0 %", itemsText: "3", modifiedText: "Today",
                            kind: .folder, ext: "", iconKey: "folder", expandable: true, expanded: true,
                            selected: sel == 0, inCloud: false, denied: false, otherVolume: false, note: "")]
            + items.map { i in
                let f = Double(i.size) / Double(total)
                return UsageRow(id: i.id, depth: 1, name: i.name, sizeText: text(i.size), size: i.size, fraction: f,
                                percentText: String(format: "%.1f %%", f * 100), itemsText: i.kind == .folder ? "0" : "",
                                modifiedText: "Today", kind: i.kind, ext: i.ext,
                                iconKey: i.kind == .folder ? "folder" : "file.\(i.ext)", expandable: false,
                                expanded: false, selected: sel == i.id, inCloud: false, denied: false,
                                otherVolume: false, note: "")
            }
        let s = Int(min(start, UInt32(all.count)))
        return Array(all[s..<min(all.count, s + Int(count))])
    }

    func setExpanded(_ id: UInt32, _ open: Bool) {}
    func rowOf(_ id: UInt32) -> UInt32? { id <= 3 ? id : nil }
    func selected() -> UInt32? { sel }
    func select(_ id: UInt32?) -> UInt32? { sel = id; return id }
    func moveSelection(_ mv: UsageMove) -> UInt32? {
        let cur = sel ?? 0
        let next: UInt32 = switch mv {
        case .up, .pageUp, .left: cur == 0 ? 0 : cur - 1
        case .down, .pageDown, .right: min(3, cur + 1)
        case .home: 0
        case .end: 3
        }
        sel = next
        return next
    }

    func info(_ id: UInt32) -> UsageNodeInfo? {
        let i = items.first { $0.id == id }
        return UsageNodeInfo(id: id, path: root + "/" + (i?.name ?? ""), name: i?.name ?? root,
                             kind: i?.kind ?? .folder, allocatedText: text(i?.size ?? total),
                             logicalText: text(i?.size ?? total), sizeText: text(i?.size ?? total), shareText: "",
                             itemsText: "", modifiedText: "Today", typeText: i?.ext.uppercased() ?? "Folder",
                             isFolder: i?.kind != .file, isRoot: i == nil, note: "")
    }

    func find(_ path: String) -> UInt32? { items.first { root + "/" + $0.name == path }?.id }
    func paths(_ ids: [UInt32]) -> [String] { ids.compactMap { id in items.first { $0.id == id }.map { root + "/" + $0.name } } }
    func zoom() -> UInt32 { zoomed }
    func setZoom(_ id: UInt32) {}
    func zoomOut() -> Bool { false }
    func zoomPath() -> [UsageLabel] { [UsageLabel(id: 0, name: (root as NSString).lastPathComponent)] }

    func layout(width: Double, height: Double) -> [UsageTile] {
        var x = 0.0
        var out = [UsageTile(id: 0, rect: CGRect(x: 0, y: 0, width: width, height: height), depth: 0, leaf: false,
                             color: 255)]
        for (n, i) in items.enumerated() {
            let w = width * Double(i.size) / Double(total)
            out.append(UsageTile(id: i.id, rect: CGRect(x: x, y: 0, width: w, height: height), depth: 1, leaf: true,
                                 color: i.kind == .folder ? 251 : UInt8(n)))
            x += w
        }
        lastWidth = width
        return out
    }

    private var lastWidth = 1.0
    func hit(x: Double, y: Double) -> UInt32? {
        var acc = 0.0
        for i in items {
            acc += lastWidth * Double(i.size) / Double(total)
            if x < acc { return i.id }
        }
        return nil
    }

    func types() -> [UsageTypeRow] {
        [UsageTypeRow(name: "MOV", ext: "mov", kindText: "Video", sizeText: text(300_000_000), size: 300_000_000,
                      countText: "1", fraction: 0.3, slot: 1),
         UsageTypeRow(name: "TXT", ext: "txt", kindText: "Document", sizeText: text(100_000_000), size: 100_000_000,
                      countText: "1", fraction: 0.1, slot: 2)]
    }

    func legend() -> [UsageLabel] { [UsageLabel(id: 1, name: "MOV"), UsageLabel(id: 2, name: "TXT")] }
    func syncPaths(_ paths: [String]) {}
}

extension MockEngine: UsageEngineAPI {
    func usageScan(path: String,
                   progress: @escaping @Sendable (UsageProgress) -> Void,
                   changed: @escaping @Sendable (UsageChange) -> Void) throws -> UsageViewAPI {
        let v = MockUsageView(root: path)
        DispatchQueue.main.async { changed(.finished) }
        return v
    }

    func usageTrash(paths: [String]) throws -> Outcome {
        Outcome(command: nil, tab: nil, shell: .moved(text: "Moved \(paths.count) items to the Trash"), ops: [],
                message: nil, pendingKeys: nil, hint: nil)
    }
}

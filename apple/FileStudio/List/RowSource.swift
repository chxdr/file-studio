// RowSource.swift — where a file table's rows come from: a tab (the list
// view, and the focused column of the column view) or a tab-less listing
// (the other columns). Both are core objects; this only unifies the calls.
// Rows are cached in blocks per listing version and dropped on every change
// (F-005): the shell never sorts or holds a second model.

import Foundation

/// A tab or a tab-less listing, as a file table sees it.
@MainActor
enum RowSource {
    case tab(TabAPI)
    case listing(ListingAPI)

    /// The id the core's events carry for this source.
    var id: TabID {
        switch self {
        case .tab(let t): t.id
        case .listing(let l): l.id
        }
    }
    var dir: String {
        switch self {
        case .tab(let t): t.location()
        case .listing(let l): l.dir()
        }
    }
    func listingInfo() -> ListingInfo {
        switch self {
        case .tab(let t): t.listingInfo()
        case .listing(let l): l.listingInfo()
        }
    }
    func rows(start: UInt32, count: UInt32) -> [Row] {
        switch self {
        case .tab(let t): t.rows(start: start, count: count)
        case .listing(let l): l.rows(start: start, count: count)
        }
    }
    func indexOf(name: String) -> UInt32? {
        switch self {
        case .tab(let t): t.indexOf(name: name)
        case .listing(let l): l.indexOf(name: name)
        }
    }
    func pathAt(_ row: UInt32) -> String? {
        switch self {
        case .tab(let t): t.pathAt(row)
        case .listing(let l): l.pathAt(row)
        }
    }
    func selection() -> Selection {
        switch self {
        case .tab(let t): t.selection()
        case .listing(let l): l.selection()
        }
    }
    func setSelection(_ c: SelectionChange) {
        switch self {
        case .tab(let t): t.setSelection(c)
        case .listing(let l): l.setSelection(c)
        }
    }
    func selectedPaths() -> [String] {
        switch self {
        case .tab(let t): t.selectedPaths()
        case .listing(let l): l.selectedPaths()
        }
    }
    func setVisibleRange(start: UInt32, count: UInt32) {
        switch self {
        case .tab(let t): t.setVisibleRange(start: start, count: count)
        case .listing(let l): l.setVisibleRange(start: start, count: count)
        }
    }
    var tab: TabAPI? { if case .tab(let t) = self { t } else { nil } }
}

/// Rows by block, for one listing version. Fetches only what is asked for.
@MainActor
final class RowCache {
    static let block: UInt32 = 64
    private var blocks: [UInt32: [Row]] = [:]
    private(set) var version: UInt64 = .max
    /// Main-thread time spent fetching rows (for `bench:scroll`).
    static var fetchTime: Duration = .zero
    static var fetchCalls = 0
    static var measuring = false

    /// Drops everything (a new listing version or a selection change).
    func invalidate(version v: UInt64) {
        blocks.removeAll(keepingCapacity: true)
        version = v
    }

    func row(_ i: Int, from src: RowSource) -> Row? {
        guard i >= 0 else { return nil }
        let start = UInt32(i) / Self.block * Self.block
        if blocks[start] == nil { fetch(start, src) }
        guard let b = blocks[start] else { return nil }
        let k = i - Int(start)
        return b.indices.contains(k) ? b[k] : nil
    }

    /// Fetches the blocks covering `range` that aren't cached yet.
    func prefetch(_ range: Range<Int>, from src: RowSource) {
        guard !range.isEmpty, range.lowerBound >= 0 else { return }
        var s = UInt32(range.lowerBound) / Self.block * Self.block
        while Int(s) < range.upperBound {
            if blocks[s] == nil { fetch(s, src) }
            s += Self.block
        }
    }

    /// A cached row (no fetch), e.g. to check the dataless flag of a path.
    func cachedRow(named name: String) -> Row? {
        for b in blocks.values { if let r = b.first(where: { $0.name == name }) { return r } }
        return nil
    }

    private func fetch(_ start: UInt32, _ src: RowSource) {
        if Self.measuring {
            let c = ContinuousClock()
            let t0 = c.now
            blocks[start] = src.rows(start: start, count: Self.block)
            Self.fetchTime += c.now - t0
            Self.fetchCalls += 1
        } else {
            blocks[start] = src.rows(start: start, count: Self.block)
        }
    }
}

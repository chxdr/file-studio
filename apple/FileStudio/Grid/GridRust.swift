// GridRust.swift — GridEngineAPI over the UniFFI bindings. Pure translation.

import Foundation

extension RustEngine: GridEngineAPI {
    func gridLayout(width: Double, iconSize: Int) -> GridLayout {
        let l = FileStudio.gridLayout(width: width, iconSize: UInt32(clamping: max(iconSize, 0)))
        return GridLayout(columns: Int(l.columns), cellWidth: l.cellWidth, cellHeight: l.cellHeight,
                          imageSize: l.imageSize, labelHeight: l.labelHeight, inset: l.inset)
    }
    func thumbPixels(points: Double, scale: Double) -> Int { Int(gridThumbPixels(points: points, scale: scale)) }
    func galleryPixels(points: Double, scale: Double) -> Int { Int(gridGalleryPixels(points: points, scale: scale)) }
    func iconSizeRange() -> ClosedRange<Int> {
        let r = gridIconSizeRange()
        return Int(r.first ?? 32)...Int(r.last ?? 256)
    }
    func setGridColumns(tab: TabID, columns: Int) {
        try? ffi.setGridColumns(tab: tab, columns: UInt32(clamping: max(columns, 1)))
    }
    func thumbnailRequests(tab: TabID, start: Int, count: Int, forward: Bool) -> [Int] {
        let r = (try? ffi.thumbnailRequests(tab: tab, start: UInt32(clamping: max(start, 0)),
                                            count: UInt32(clamping: max(count, 0)), forward: forward)) ?? []
        return r.map(Int.init)
    }
    func galleryRequests(tab: TabID, forward: Bool) -> [Int] {
        ((try? ffi.galleryRequests(tab: tab, forward: forward)) ?? []).map(Int.init)
    }
    func setIconSize(_ size: Int) { try? ffi.setIconSize(size: UInt32(clamping: max(size, 0))) }
}

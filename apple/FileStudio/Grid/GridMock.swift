// GridMock.swift — GridEngineAPI for MockCore runs (no Rust). Rough stand-ins
// so the views can be drawn without the core; the real rules are in
// `fstudio-core/src/grid.rs`.

import Foundation

extension MockEngine: GridEngineAPI {
    func gridLayout(width: Double, iconSize: Int) -> GridLayout {
        let image = Double(min(max(iconSize, 32), 256))
        let minCell = max(image + 24, 92)
        let avail = max(width - 24, minCell)
        let cols = max(Int(avail / minCell), 1)
        return GridLayout(columns: cols, cellWidth: avail / Double(cols), cellHeight: image + 48, imageSize: image,
                          labelHeight: 32, inset: 12)
    }
    func thumbPixels(points: Double, scale: Double) -> Int { 256 }
    func galleryPixels(points: Double, scale: Double) -> Int { 2048 }
    func iconSizeRange() -> ClosedRange<Int> { 32...256 }
    func setGridColumns(tab: TabID, columns: Int) {}
    func thumbnailRequests(tab: TabID, start: Int, count: Int, forward: Bool) -> [Int] { [] }
    func galleryRequests(tab: TabID, forward: Bool) -> [Int] { [] }
    func setIconSize(_ size: Int) {}
}

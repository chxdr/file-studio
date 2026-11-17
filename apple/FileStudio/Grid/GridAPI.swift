// GridAPI.swift — the icon and gallery views' contract with the core (M3).
//
// The core decides the layout (items per row, tile size), where the arrow
// keys go (`handle_key` already does it: the shell only reports how many
// items fit in a row), the thumbnail size and its steps (`grid.icon_size`,
// ⇧⌘= / ⇧⌘-), the pixel sizes asked for, and the order thumbnails are made
// in (cursor, screen, then what scrolls in next; never iCloud placeholders,
// F-016). These views only draw and ask QuickLook (Thumbnails.swift's job
// for the preview pane; ThumbnailLoader.swift here).

import Foundation

/// Where the tiles of the icon view go, in points.
public struct GridLayout: Equatable, Sendable {
    public var columns: Int
    public var cellWidth: Double
    public var cellHeight: Double
    public var imageSize: Double
    public var labelHeight: Double
    public var inset: Double
    public init(columns: Int, cellWidth: Double, cellHeight: Double, imageSize: Double, labelHeight: Double,
                inset: Double) {
        self.columns = columns; self.cellWidth = cellWidth; self.cellHeight = cellHeight
        self.imageSize = imageSize; self.labelHeight = labelHeight; self.inset = inset
    }
}

/// The grid calls on the engine. `RustEngine` and `MockEngine` adopt it.
public protocol GridEngineAPI: AnyObject, Sendable {
    /// Lays out the icon view for a content width and a thumbnail size.
    func gridLayout(width: Double, iconSize: Int) -> GridLayout
    /// The pixel size to ask QuickLook for (a few fixed steps).
    func thumbPixels(points: Double, scale: Double) -> Int
    /// The pixel size (longer side) of the gallery's big picture.
    func galleryPixels(points: Double, scale: Double) -> Int
    /// The slider's range, in points.
    func iconSizeRange() -> ClosedRange<Int>
    /// The shell laid out the icon view of `tab` with `columns` per row.
    func setGridColumns(tab: TabID, columns: Int)
    /// Thumbnails to ask for, in order, while items `start..<start+count` are on screen.
    func thumbnailRequests(tab: TabID, start: Int, count: Int, forward: Bool) -> [Int]
    /// The gallery's big pictures to make, in order, around the cursor.
    func galleryRequests(tab: TabID, forward: Bool) -> [Int]
    /// The size slider.
    func setIconSize(_ size: Int)
}

/// Icon view and gallery settings (`grid.*`), read from the core.
@MainActor
enum GridSettings {
    static func iconSize(_ m: AppModel) -> Int { Core.int(m.engine, "grid.icon_size") ?? 128 }
    static func stripSize(_ m: AppModel) -> Int { Core.int(m.engine, "grid.strip_size") ?? 72 }
    static func galleryInfo(_ m: AppModel) -> Bool { Core.bool(m.engine, "grid.gallery_info") ?? true }
}

/// Import Studio culling marks on a tile: stars, a colour label, pick or reject.
/// Drawing only; the core reads the sidecars.
struct GridMarks: Equatable {
    /// 0 unrated, 1–5 stars.
    var rating: Int
    /// 1 red, 2 yellow, 3 green, 4 blue, 5 purple; nil for none.
    var label: Int?
    var reject: Bool
    var pick: Bool

    /// The marks of a row.
    ///
    /// TODO(culling-marks): return the row's marks once branch
    /// `claude/laughing-shaw-98f473` ("Show Import Studio ratings, labels and
    /// culls") is merged into main. It adds `Row.marks: CullMarks?`; the body
    /// becomes:
    ///   guard let m = r.marks else { return nil }
    ///   return GridMarks(rating: Int(m.rating), label: m.label.map { Int($0.rawValue) },
    ///                    reject: m.flag == .reject, pick: m.flag == .pick)
    @MainActor
    static func of(_ r: Row) -> GridMarks? {
        #if DEBUG
        if debugFake { return fake(r.name) }
        #endif
        return nil
    }

    #if DEBUG
    /// Debug step `gridmarks:fake`: made-up marks from the name, to check the drawing.
    @MainActor static var debugFake = false
    static func fake(_ name: String) -> GridMarks? {
        let h = name.unicodeScalars.reduce(UInt32(7)) { ($0 &* 31) &+ $1.value }
        guard h % 4 != 0 else { return nil }
        return GridMarks(rating: Int(h % 6), label: h % 3 == 0 ? Int(h % 5) + 1 : nil, reject: h % 7 == 0,
                         pick: h % 11 == 0)
    }
    #endif
}

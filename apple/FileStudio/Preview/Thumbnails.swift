// Thumbnails.swift — pictures for the preview pane through QuickLook
// (QLThumbnailGenerator). It handles JPEG, HEIC and the camera raws the
// system decodes (CR3, NEF, ARW, DNG…) and movie frames. This is an OS call,
// so it lives in the shell. It is never asked about an iCloud placeholder
// (the core's `dataless` flag is checked first), because a thumbnail request
// would download the file (F-016).

import AppKit
import QuickLookThumbnailing

/// A finished picture, boxed so it can cross from the generator's queue.
struct ThumbnailBox: @unchecked Sendable { let image: NSImage }
private struct RequestBox: @unchecked Sendable { let request: QLThumbnailGenerator.Request }

/// One request at a time: a new one cancels the old.
@MainActor
final class Thumbnails {
    private var current: QLThumbnailGenerator.Request?
    private let cache = NSCache<NSString, NSImage>()

    init() { cache.countLimit = 120 }

    static func key(_ info: PreviewInfo, width: Int) -> NSString {
        "\(info.path)|\(info.modifiedNs)|\(info.size)|\(width)" as NSString
    }

    func cached(_ info: PreviewInfo, width: Int) -> NSImage? { cache.object(forKey: Self.key(info, width: width)) }

    func cancel() {
        if let r = current { QLThumbnailGenerator.shared.cancel(r) }
        current = nil
    }

    /// Asks for a picture of `info` at most `size` points. `update` gets the
    /// quick low-quality one first, then the proper one, on the main actor.
    func request(_ info: PreviewInfo, size: CGSize, scale: CGFloat,
                 update: @escaping @MainActor @Sendable (NSImage, Bool) -> Void) {
        cancel()
        let url = URL(fileURLWithPath: info.path)
        let req = QLThumbnailGenerator.Request(fileAt: url, size: size, scale: scale,
                                               representationTypes: [.lowQualityThumbnail, .thumbnail])
        current = req
        let key: String = Self.key(info, width: Int(size.width)) as String
        let reqBox = RequestBox(request: req)
        // Called on the generator's queue, so this closure must not be main-actor isolated.
        QLThumbnailGenerator.shared.generateRepresentations(for: req) { @Sendable [weak self] rep, type, _ in
            guard let rep else { return }
            let box = ThumbnailBox(image: NSImage(cgImage: rep.cgImage, size: rep.contentRect.size == .zero ? size : NSSize(
                width: CGFloat(rep.cgImage.width) / scale, height: CGFloat(rep.cgImage.height) / scale)))
            let final = type == .thumbnail
            DispatchQueue.main.async {
                MainActor.assumeIsolated {
                    guard let self, self.current === reqBox.request else { return }
                    if final { self.cache.setObject(box.image, forKey: key as NSString) }
                    update(box.image, final)
                }
            }
        }
    }
}

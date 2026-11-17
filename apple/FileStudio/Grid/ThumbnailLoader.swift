// ThumbnailLoader.swift — QuickLook thumbnails for the icon view and the
// gallery, made off the main thread and cached.
//
// The core says which items to make and in what order (`thumbnailRequests`,
// `galleryRequests`) and at which pixel size; this keeps a queue in that
// order, runs a few QuickLook requests at a time, cancels work that has
// dropped out of the core's list, and caches the pictures by
// (path, modified, size, pixels) in one cache shared by every view. It is
// never given an iCloud placeholder (the core leaves them out, F-016).

import AppKit
import ImageIO
import QuickLookThumbnailing

/// A finished picture, boxed so it can cross from QuickLook's queue.
final class GridImage: @unchecked Sendable {
    let cg: CGImage
    init(_ cg: CGImage) { self.cg = cg }
    var cost: Int { cg.bytesPerRow * cg.height }
}

/// One picture to make.
struct ThumbRequest: Equatable {
    /// The cache key: path, modified time, size and pixel size.
    let key: String
    let path: String
    /// Longer side, in pixels.
    let pixels: Int
    /// Show QuickLook's quick low-quality picture first (the gallery).
    var quickFirst = false

    static func key(path: String, modifiedNs: Int64, size: UInt64, pixels: Int) -> String {
        "\(path)|\(modifiedNs)|\(size)|\(pixels)"
    }
}

@MainActor
final class ThumbnailLoader {
    /// Shared by every grid and gallery: switching views keeps the pictures.
    private static let cache: NSCache<NSString, GridImage> = {
        let c = NSCache<NSString, GridImage>()
        c.totalCostLimit = 384 << 20
        return c
    }()
    /// Keys QuickLook had no picture for (the view keeps the icon).
    private static var failed = Set<String>()
    /// Low-quality pictures, by the key of the proper one (gallery only).
    private static let quick: NSCache<NSString, GridImage> = {
        let c = NSCache<NSString, GridImage>()
        c.countLimit = 12
        return c
    }()

    /// Requests in the core's order, not started yet.
    private var queue: [ThumbRequest] = []
    private var inflight: [String: Job] = [:]
    private let maxInflight: Int
    /// A picture arrived (`final` false: the quick low-quality one).
    var onImage: ((String, GridImage, _ final: Bool) -> Void)?

    /// Made so far, for the debug log.
    private(set) var made = 0
    private(set) var cancelled = 0
    private(set) var fromImageIO = 0

    /// One picture being made: QuickLook first, then ImageIO's embedded preview.
    final class Job: @unchecked Sendable {
        let request: QLThumbnailGenerator.Request
        private let lock = NSLock()
        private var _cancelled = false
        init(_ r: QLThumbnailGenerator.Request) { request = r }
        var isCancelled: Bool { lock.lock(); defer { lock.unlock() }; return _cancelled }
        func cancel() {
            lock.lock(); _cancelled = true; lock.unlock()
            QLThumbnailGenerator.shared.cancel(request)
        }
    }

    /// Embedded previews and fallbacks are read here, a few at a time, never on the main thread.
    private static let imageIOQueue = DispatchQueue(label: "grid.thumbs.imageio", qos: .userInitiated,
                                                    attributes: .concurrent)

    init(maxInflight: Int = 6) { self.maxInflight = maxInflight }

    /// The cached picture for a key, if made.
    static func image(_ key: String) -> GridImage? { cache.object(forKey: key as NSString) }
    static func quickImage(_ key: String) -> GridImage? { quick.object(forKey: key as NSString) }
    static func hasFailed(_ key: String) -> Bool { failed.contains(key) }

    /// Replaces what is wanted, in priority order. Work no longer wanted is
    /// cancelled; cached and failed keys are skipped.
    func want(_ reqs: [ThumbRequest]) {
        var seen = Set<String>()
        queue = reqs.filter { r in
            guard seen.insert(r.key).inserted else { return false }
            return Self.cache.object(forKey: r.key as NSString) == nil && !Self.failed.contains(r.key)
                && inflight[r.key] == nil
        }
        for (k, j) in inflight where !seen.contains(k) {
            j.cancel()
            inflight[k] = nil
            cancelled += 1
        }
        pump()
    }

    /// Stops everything (the view went away or changed folder).
    func cancelAll() {
        queue.removeAll()
        for j in inflight.values { j.cancel() }
        cancelled += inflight.count
        inflight.removeAll()
    }

    var pending: Int { queue.count + inflight.count }

    private func pump() {
        while inflight.count < maxInflight, !queue.isEmpty {
            start(queue.removeFirst())
        }
    }

    private func start(_ r: ThumbRequest) {
        let scale: CGFloat = 2
        let side = CGFloat(r.pixels) / scale
        let types: QLThumbnailGenerator.Request.RepresentationTypes = r.quickFirst ? [.lowQualityThumbnail, .thumbnail] : .thumbnail
        let req = QLThumbnailGenerator.Request(fileAt: URL(fileURLWithPath: r.path), size: CGSize(width: side, height: side),
                                               scale: scale, representationTypes: types)
        let job = Job(req)
        inflight[r.key] = job
        let key = r.key, path = r.path, pixels = r.pixels
        let quickFirst = r.quickFirst
        QLThumbnailGenerator.shared.generateRepresentations(for: req) { @Sendable [weak self] rep, type, _ in
            // The proper picture, or a failure of a request that only asked for it.
            let final = type == .thumbnail || (rep == nil && !quickFirst)
            if rep == nil, !final { return }
            if let rep {
                let img = GridImage(rep.cgImage)
                DispatchQueue.main.async {
                    MainActor.assumeIsolated { self?.arrived(key: key, job: job, image: img, final: final, viaImageIO: false) }
                }
                return
            }
            // QuickLook won't make it (camera raws stop at their embedded
            // preview's size): ask ImageIO for the embedded preview instead.
            guard !job.isCancelled else { return }
            Self.imageIOQueue.async {
                let img = job.isCancelled ? nil : Self.embeddedPreview(path, pixels: pixels)
                DispatchQueue.main.async {
                    MainActor.assumeIsolated { self?.arrived(key: key, job: job, image: img, final: true, viaImageIO: true) }
                }
            }
        }
    }

    /// The file's own preview (a raw's embedded JPEG), or the image itself
    /// scaled down, with its orientation applied. Reads the file; never
    /// called for an iCloud placeholder (the core leaves them out).
    nonisolated static func embeddedPreview(_ path: String, pixels: Int) -> GridImage? {
        guard let src = CGImageSourceCreateWithURL(URL(fileURLWithPath: path) as CFURL,
                                                   [kCGImageSourceShouldCache: false] as CFDictionary) else { return nil }
        let opts: [CFString: Any] = [
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceCreateThumbnailFromImageIfAbsent: true,
            kCGImageSourceThumbnailMaxPixelSize: pixels,
            kCGImageSourceShouldCacheImmediately: true,
        ]
        return CGImageSourceCreateThumbnailAtIndex(src, 0, opts as CFDictionary).map(GridImage.init)
    }

    private func arrived(key: String, job: Job, image: GridImage?, final: Bool, viaImageIO: Bool) {
        guard inflight[key] === job else { return }
        if let image {
            if final {
                Self.cache.setObject(image, forKey: key as NSString, cost: image.cost)
                made += 1
                if viaImageIO { fromImageIO += 1 }
            } else {
                Self.quick.setObject(image, forKey: key as NSString)
            }
            onImage?(key, image, final)
        } else if final {
            Self.failed.insert(key)
        }
        if final {
            inflight[key] = nil
            pump()
        }
    }
}

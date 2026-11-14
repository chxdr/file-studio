// PreviewModel.swift — what the preview pane shows, and how it keeps up.
//
// The pane follows the active tab's cursor (or its folder when nothing is
// selected). Each move cancels the work for the item before it, waits a
// moment so holding ↓ through a folder of raws never queues disk work, then
// asks the core for the summary on a background task. Summaries are cached
// in the core by (path, mtime, size); thumbnails by the same in the shell.
// Nothing here reads a file on the main thread.

import AppKit
import Observation

@MainActor
@Observable
final class PreviewModel {
    /// The pane is shown (mirrors the `preview.visible` setting).
    var visible = false {
        didSet { if visible != oldValue { refresh() } }
    }
    /// The item being shown, once its summary has arrived.
    private(set) var info: PreviewInfo?
    /// The system icon for the item, shown until a picture arrives.
    private(set) var icon: NSImage?
    /// The QuickLook picture.
    private(set) var picture: NSImage?
    /// Waiting for the summary.
    private(set) var loading = false
    /// How many items are selected (the pane shows the cursor item).
    private(set) var selectionCount = 0
    /// Paths whose download was asked for from this pane.
    private(set) var downloading: Set<String> = []
    /// Settings → Previews.
    var showThumbnails = true
    var highlight = true

    @ObservationIgnored private let previewer: PreviewerAPI
    @ObservationIgnored private let thumbs = Thumbnails()
    @ObservationIgnored private var wanted: String?
    @ObservationIgnored private var shownPath: String?
    @ObservationIgnored private var task: Task<Void, Never>?
    @ObservationIgnored private var cancel: PreviewCancel?
    @ObservationIgnored var scale: CGFloat = 2
    /// How wide the picture may be, in points (set from the pane's width).
    @ObservationIgnored var pictureWidth: CGFloat = 276
    /// Summaries made (not shown), for the debug proof.
    @ObservationIgnored private(set) var requests = 0

    init(previewer: PreviewerAPI) { self.previewer = previewer }

    /// Follows a new item. `path` is nil when there is nothing to show.
    func follow(path: String?, selectionCount: Int) {
        self.selectionCount = selectionCount
        guard path != wanted || path == nil else { return }
        wanted = path
        refresh()
    }

    /// Shows the item again (the file changed, or the setting did).
    func refresh(force: Bool = false) {
        task?.cancel()
        cancel?.cancel()
        thumbs.cancel()
        guard visible, let path = wanted else {
            info = nil; icon = nil; picture = nil; loading = false; shownPath = nil
            return
        }
        if force || shownPath != path { loading = shownPath != path || info == nil }
        let token = PreviewCancel()
        cancel = token
        let previewer = self.previewer
        task = Task { [weak self] in
            // Hold ↓ through a folder: only the item you stop on is read.
            try? await Task.sleep(for: .milliseconds(force ? 0 : 45))
            if Task.isCancelled { return }
            let result: PreviewInfo? = await withTaskCancellationHandler {
                await Task.detached(priority: .userInitiated) {
                    guard var i = previewer.summarize(path, cancel: token) else { return nil }
                    ImageSize.fill(&i)
                    return i
                }.value
            } onCancel: { token.cancel() }
            if Task.isCancelled { return }
            self?.arrived(path: path, info: result)
        }
    }

    private func arrived(path: String, info i: PreviewInfo?) {
        guard path == wanted else { return }
        requests += 1
        loading = false
        guard let i else {
            info = nil; icon = nil; picture = nil; shownPath = path
            return
        }
        if shownPath != path || icon == nil { icon = NSWorkspace.shared.icon(forFile: path) }
        shownPath = path
        info = i
        picture = nil
        downloading.remove(path)
        guard showThumbnails, !i.dataless, Self.wantsPicture(i.category) else { return }
        let w = Int(pictureWidth)
        if let c = thumbs.cached(i, width: w) { picture = c; return }
        thumbs.request(i, size: CGSize(width: pictureWidth, height: 220), scale: scale) { [weak self] img, _ in
            guard let self, self.info?.path == i.path else { return }
            self.picture = img
        }
    }

    /// Folders, packages and text show their own content instead of a picture.
    private static func wantsPicture(_ c: PreviewCategory) -> Bool {
        switch c {
        case .image, .raw, .video, .audio, .other, .unknown: true
        case .text, .folder, .package: false
        }
    }

    /// Asks iCloud to download a placeholder. The pane never does this by itself.
    func download() {
        guard let i = info, i.dataless else { return }
        downloading.insert(i.path)
        do { try FileManager.default.startDownloadingUbiquitousItem(at: URL(fileURLWithPath: i.path)) } catch {
            downloading.remove(i.path)
        }
    }

    /// Debug: what the pane knows.
    var debugLine: [String: Any] {
        ["visible": visible, "path": shownPath ?? "", "loading": loading, "hasPicture": picture != nil,
         "category": info.map { "\($0.category)" } ?? "", "dataless": info?.dataless ?? false,
         "camera": info?.photo?.camera ?? "", "dims": info?.photo?.dimensions ?? "",
         "lines": info?.text?.lines.count ?? 0, "requests": requests, "selected": selectionCount,
         "note": info?.note ?? ""]
    }
}

/// Fills a photo's pixel size from ImageIO when the file's EXIF has none
/// (a header read; never called for placeholders).
enum ImageSize {
    static func fill(_ i: inout PreviewInfo) {
        guard !i.dataless, i.category == .image || i.category == .raw else { return }
        if i.photo?.dimensions != nil { return }
        guard let src = CGImageSourceCreateWithURL(URL(fileURLWithPath: i.path) as CFURL, nil),
              let p = CGImageSourceCopyPropertiesAtIndex(src, 0, nil) as? [CFString: Any],
              let w = p[kCGImagePropertyPixelWidth] as? Int, let h = p[kCGImagePropertyPixelHeight] as? Int,
              w > 0, h > 0 else { return }
        var photo = i.photo ?? PreviewPhoto()
        photo.width = w; photo.height = h; photo.dimensions = "\(w) × \(h)"
        i.photo = photo
    }
}

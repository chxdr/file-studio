// PreviewAPI.swift — the preview pane's contract with the core (M3).
//
// Plain value types mirroring fstudio-preview's records, and the protocol the
// shell uses. `summarize` blocks on disk: call it from a background task and
// cancel it with `PreviewCancel` when the cursor moves on. Metadata comes from
// the core; pictures come from QuickLook (Thumbnails.swift).

import Foundation

public enum PreviewCategory: Sendable, Equatable {
    case image, raw, video, audio, text, folder, package, other, unknown
}

/// What kind of token a highlighted span is; the theme picks the colour.
public enum TokenClass: Sendable, Equatable {
    case keyword, string, number, comment, function, type, constant, punctuation, tag, attribute, heading, variable, invalid
}

public struct PreviewTag: Sendable, Equatable {
    public var name: String
    /// Finder colour index 0–7 (none, gray, green, purple, blue, yellow, red, orange).
    public var color: Int
    public init(name: String, color: Int) { self.name = name; self.color = color }
}

public struct PreviewPhoto: Sendable, Equatable {
    public var dateTaken: String?
    public var camera: String?
    public var lens: String?
    public var focalLength: String?
    public var aperture: String?
    public var shutter: String?
    public var iso: String?
    public var exposureComp: String?
    public var dimensions: String?
    public var width: Int?
    public var height: Int?
    public var orientation: Int?
    public init() {}
}

public struct PreviewVideo: Sendable, Equatable {
    public var codec: String?
    public var duration: String?
    public var resolution: String?
    public var frameRate: String?
    public var audio: String?
    public init() {}
}

/// A coloured run in a line, in UTF-16 code units.
public struct PreviewSpan: Sendable, Equatable {
    public var start: Int
    public var len: Int
    public var kind: TokenClass
    public init(start: Int, len: Int, kind: TokenClass) { self.start = start; self.len = len; self.kind = kind }
}

public struct PreviewTextLine: Sendable, Equatable {
    public var text: String
    public var spans: [PreviewSpan]
    public init(text: String, spans: [PreviewSpan] = []) { self.text = text; self.spans = spans }
}

public struct PreviewText: Sendable, Equatable {
    public var encoding: String
    public var language: String?
    public var lineCount: UInt64
    public var linesExact: Bool
    public var lines: [PreviewTextLine]
    public var truncated: Bool
    public init(encoding: String, language: String?, lineCount: UInt64, linesExact: Bool,
                lines: [PreviewTextLine], truncated: Bool) {
        self.encoding = encoding; self.language = language; self.lineCount = lineCount
        self.linesExact = linesExact; self.lines = lines; self.truncated = truncated
    }
}

public struct PreviewFolder: Sendable, Equatable {
    public var items: UInt64
    public var atLeast: Bool
    public var unreadable: Bool
    public init(items: UInt64, atLeast: Bool, unreadable: Bool) {
        self.items = items; self.atLeast = atLeast; self.unreadable = unreadable
    }
}

/// Everything the pane shows about one item.
public struct PreviewInfo: Sendable, Equatable {
    public var path: String
    /// The name exactly as stored (never normalised).
    public var name: String
    public var category: PreviewCategory
    public var size: UInt64
    public var modifiedNs: Int64
    public var createdNs: Int64?
    public var accessedNs: Int64?
    public var isSymlink: Bool
    /// An iCloud placeholder: nothing was read; the pane offers Download.
    public var dataless: Bool
    public var tags: [PreviewTag]
    public var photo: PreviewPhoto?
    public var video: PreviewVideo?
    public var text: PreviewText?
    public var folder: PreviewFolder?
    public var note: String?

    public init(path: String, name: String, category: PreviewCategory, size: UInt64, modifiedNs: Int64) {
        self.path = path; self.name = name; self.category = category; self.size = size; self.modifiedNs = modifiedNs
        isSymlink = false; dataless = false; tags = []
    }
}

/// Cancels one `summarize` call. Any thread.
public final class PreviewCancel: @unchecked Sendable {
    private let lock = NSLock()
    private var flag = false
    private var hooks: [@Sendable () -> Void] = []
    public init() {}

    public var isCancelled: Bool { lock.lock(); defer { lock.unlock() }; return flag }

    public func cancel() {
        lock.lock()
        flag = true
        let h = hooks
        hooks = []
        lock.unlock()
        h.forEach { $0() }
    }

    /// Runs `fn` when cancelled (at once if it already was).
    public func onCancel(_ fn: @escaping @Sendable () -> Void) {
        lock.lock()
        if flag { lock.unlock(); fn(); return }
        hooks.append(fn)
        lock.unlock()
    }
}

public protocol PreviewerAPI: AnyObject, Sendable {
    /// The preview of `path`. **Blocks on disk.** `nil` when cancelled or unreadable.
    func summarize(_ path: String, cancel: PreviewCancel) -> PreviewInfo?
    /// The cached preview if the file is unchanged. Reads no content.
    func cached(_ path: String) -> PreviewInfo?
    func clear()
}

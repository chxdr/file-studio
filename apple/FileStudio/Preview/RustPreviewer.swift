// RustPreviewer.swift — PreviewerAPI on the UniFFI bindings. Pure translation.

import Foundation

final class RustPreviewer: PreviewerAPI, @unchecked Sendable {
    private let ffi: FfiPreviewer

    init(engine: FfiEngine) { ffi = FfiPreviewer(engine: engine) }

    func summarize(_ path: String, cancel: PreviewCancel) -> PreviewInfo? {
        let token = FfiCancelToken()
        cancel.onCancel { token.cancel() }
        return ffi.summarize(path: path, cancel: token).map(PreviewInfo.init)
    }

    func cached(_ path: String) -> PreviewInfo? { ffi.cached(path: path).map(PreviewInfo.init) }
    func clear() { ffi.clear() }
}

extension PreviewInfo {
    init(_ f: FfiPreview) {
        let cat: PreviewCategory = switch f.category {
        case .image: .image
        case .raw: .raw
        case .video: .video
        case .audio: .audio
        case .text: .text
        case .folder: .folder
        case .package: .package
        case .other: .other
        case .unknown: .unknown
        }
        self.init(path: f.path, name: f.name, category: cat, size: f.size, modifiedNs: f.modifiedNs)
        createdNs = f.createdNs
        accessedNs = f.accessedNs
        isSymlink = f.isSymlink
        dataless = f.dataless
        tags = f.tags.map { PreviewTag(name: $0.name, color: Int($0.color)) }
        note = f.note
        marks = f.marks.map(CullMarks.init)
        photo = f.photo.map { p in
            var o = PreviewPhoto()
            o.dateTaken = p.dateTaken; o.camera = p.camera; o.lens = p.lens
            o.focalLength = p.focalLength; o.aperture = p.aperture; o.shutter = p.shutter
            o.iso = p.iso; o.exposureComp = p.exposureComp; o.dimensions = p.dimensions
            o.width = p.width.map(Int.init); o.height = p.height.map(Int.init)
            o.orientation = p.orientation.map(Int.init)
            return o
        }
        video = f.video.map { v in
            var o = PreviewVideo()
            o.codec = v.codec; o.duration = v.duration; o.resolution = v.resolution
            o.frameRate = v.frameRate; o.audio = v.audio
            return o
        }
        text = f.text.map { t in
            PreviewText(encoding: t.encoding, language: t.language, lineCount: t.lineCount,
                        linesExact: t.linesExact,
                        lines: t.lines.map { l in
                            PreviewTextLine(text: l.text, spans: l.spans.map {
                                PreviewSpan(start: Int($0.start), len: Int($0.len), kind: TokenClass($0.class))
                            })
                        },
                        truncated: t.truncated)
        }
        folder = f.folder.map { PreviewFolder(items: $0.items, atLeast: $0.atLeast, unreadable: $0.unreadable) }
    }
}

extension TokenClass {
    init(_ c: FfiTokenClass) {
        self = switch c {
        case .keyword: .keyword
        case .string: .string
        case .number: .number
        case .comment: .comment
        case .function: .function
        case .type: .type
        case .constant: .constant
        case .punctuation: .punctuation
        case .tag: .tag
        case .attribute: .attribute
        case .heading: .heading
        case .variable: .variable
        case .invalid: .invalid
        }
    }
}

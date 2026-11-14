// MockPreviewer.swift — a previewer for MockCore (no Rust): just the name and
// whatever `FileManager` knows, so the pane can be laid out against the mock.

import Foundation

final class MockPreviewer: PreviewerAPI, @unchecked Sendable {
    func summarize(_ path: String, cancel: PreviewCancel) -> PreviewInfo? {
        let name = (path as NSString).lastPathComponent
        let attrs = try? FileManager.default.attributesOfItem(atPath: path)
        let isDir = (attrs?[.type] as? FileAttributeType) == .typeDirectory
        var i = PreviewInfo(path: path, name: name, category: isDir ? .folder : .unknown,
                            size: (attrs?[.size] as? UInt64) ?? 0,
                            modifiedNs: Int64(((attrs?[.modificationDate] as? Date)?.timeIntervalSince1970 ?? 0) * 1e9))
        if isDir { i.folder = PreviewFolder(items: 0, atLeast: false, unreadable: false) }
        return i
    }
    func cached(_ path: String) -> PreviewInfo? { nil }
    func clear() {}
}

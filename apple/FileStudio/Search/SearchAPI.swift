// SearchAPI.swift — M3 search, the shell's view of it. A search runs in a
// tab; its results are the tab's rows (the list view, sorting, selection,
// Quick Look and file operations work on them as on a folder). This file
// adds the search calls to the tab API and converts the FFI records. All
// search logic (sources, filters, merging, status text) is in Rust
// (`fstudio-search`, `fstudio-core/src/tab_search.rs`).

import Foundation

public enum SearchScope: Equatable, Sendable { case thisFolder, thisMac }

public enum SearchNameMode: Equatable, Sendable, CaseIterable {
    case contains, glob, regex
    var title: String {
        switch self {
        case .contains: "Name Contains"
        case .glob: "Name Matches Pattern (*.png)"
        case .regex: "Name Matches Regular Expression"
        }
    }
}

public enum SearchKind: Equatable, Sendable, CaseIterable {
    case folder, application, image, movie, audio, pdf, text, document, archive, other
    var title: String {
        switch self {
        case .folder: "Folder"
        case .application: "Application"
        case .image: "Image"
        case .movie: "Movie"
        case .audio: "Music & Audio"
        case .pdf: "PDF"
        case .text: "Text & Code"
        case .document: "Document"
        case .archive: "Archive & Disk Image"
        case .other: "Other"
        }
    }
}

public enum SearchDateField: Equatable, Sendable { case modified, created }

/// What to search for. Unset filters are nil or empty.
public struct SearchRequest: Equatable, Sendable {
    public var scope: SearchScope = .thisFolder
    public var name = ""
    public var nameMode: SearchNameMode = .contains
    public var kinds: [SearchKind] = []
    public var minSize: UInt64?
    public var maxSize: UInt64?
    public var dateField: SearchDateField = .modified
    public var dateFromNs: Int64?
    public var dateToNs: Int64?
    public var tags: [String] = []
    /// Text the file must contain (text files only); nil is off.
    public var content: String?
    public var includeHidden = false
    public var includePackages = false

    /// Nothing narrows it (every item would match).
    public var isUnfiltered: Bool {
        name.isEmpty && kinds.isEmpty && minSize == nil && maxSize == nil && dateFromNs == nil && dateToNs == nil
            && tags.isEmpty && (content ?? "").isEmpty
    }
}

public enum SearchState: Equatable, Sendable { case searching, done, stopped, truncated }

/// A search tab's status, worded by the core.
public struct SearchStatus: Equatable, Sendable {
    public var request: SearchRequest
    /// The folder the tab showed before (Back returns there).
    public var folder: String
    public var root: String
    public var state: SearchState
    public var found: UInt64
    public var dirsScanned: UInt64
    public var denied: [String]
    public var deniedCount: UInt64
    public var cloudSkipped: UInt64
    public var indexUsed: Bool
    public var indexError: String?
    public var elapsedMs: UInt64
    public var firstHitMs: UInt64?
    /// "Searching… 120 found", "1,204 found in 2.1 s".
    public var text: String
}

/// The search calls on a tab.
public protocol SearchTabAPI: AnyObject {
    /// Runs a search; results replace the tab's rows as they are found.
    func search(_ r: SearchRequest) throws
    /// Stops a running search, keeping its results.
    @discardableResult func stopSearch() -> Bool
    /// Ends the search and shows the folder it started from.
    @discardableResult func endSearch() -> Bool
    /// The status; nil for a folder tab.
    func searchStatus() -> SearchStatus?
}

// MARK: - Rust

extension RustTab: SearchTabAPI {
    func search(_ r: SearchRequest) throws {
        do { try ffi.search(request: r.ffi) } catch let e as FfiError { throw CoreError(search: e) }
    }
    func stopSearch() -> Bool { ffi.stopSearch() }
    func endSearch() -> Bool { ffi.endSearch() }
    func searchStatus() -> SearchStatus? { ffi.searchStatus().map(SearchStatus.init) }
}

extension CoreError {
    init(search e: FfiError) {
        switch e {
        case .NotFound(let m): self.init(.notFound, m)
        case .NotAvailable(let m): self.init(.notAvailable, m)
        case .InvalidName(let m): self.init(.invalidName, m)
        case .InvalidSetting(let m): self.init(.invalidSetting, m)
        case .UnknownCommand(let m): self.init(.unknownCommand, m)
        case .Failed(let m): self.init(.failed, m)
        }
    }
}

private extension SearchKind {
    var ffi: FfiSearchKind {
        switch self {
        case .folder: .folder
        case .application: .application
        case .image: .image
        case .movie: .movie
        case .audio: .audio
        case .pdf: .pdf
        case .text: .text
        case .document: .document
        case .archive: .archive
        case .other: .other
        }
    }
    init(_ k: FfiSearchKind) {
        self = switch k {
        case .folder: .folder
        case .application: .application
        case .image: .image
        case .movie: .movie
        case .audio: .audio
        case .pdf: .pdf
        case .text: .text
        case .document: .document
        case .archive: .archive
        case .other: .other
        }
    }
}

extension SearchRequest {
    var ffi: FfiSearchRequest {
        let m: FfiNameMode = switch nameMode { case .contains: .contains; case .glob: .glob; case .regex: .regex }
        return FfiSearchRequest(
            scope: scope == .thisMac ? .thisMac : .thisFolder, name: name,
            nameMode: m,
            kinds: kinds.map(\.ffi), minSize: minSize, maxSize: maxSize,
            dateField: dateField == .created ? .created : .modified, dateFromNs: dateFromNs, dateToNs: dateToNs,
            tags: tags, content: content, includeHidden: includeHidden, includePackages: includePackages)
    }
    init(_ f: FfiSearchRequest) {
        self.init()
        scope = f.scope == .thisMac ? .thisMac : .thisFolder
        name = f.name
        nameMode = switch f.nameMode { case .contains: .contains; case .glob: .glob; case .regex: .regex }
        kinds = f.kinds.map(SearchKind.init)
        minSize = f.minSize; maxSize = f.maxSize
        dateField = f.dateField == .created ? .created : .modified
        dateFromNs = f.dateFromNs; dateToNs = f.dateToNs
        tags = f.tags; content = f.content
        includeHidden = f.includeHidden; includePackages = f.includePackages
    }
}

extension SearchStatus {
    init(_ f: FfiSearchStatus) {
        let st: SearchState = switch f.state {
        case .searching: .searching
        case .done: .done
        case .stopped: .stopped
        case .truncated: .truncated
        }
        self.init(request: SearchRequest(f.request), folder: f.folder, root: f.root,
                  state: st,
                  found: f.found, dirsScanned: f.dirsScanned, denied: f.denied, deniedCount: f.deniedCount,
                  cloudSkipped: f.cloudSkipped, indexUsed: f.indexUsed, indexError: f.indexError,
                  elapsedMs: f.elapsedMs, firstHitMs: f.firstHitMs, text: f.text)
    }
}

// MARK: - Mock

extension MockTab: SearchTabAPI {
    func search(_ r: SearchRequest) throws {
        throw CoreError(.notAvailable, "Search needs the Rust core")
    }
    func stopSearch() -> Bool { false }
    func endSearch() -> Bool { false }
    func searchStatus() -> SearchStatus? { nil }
}

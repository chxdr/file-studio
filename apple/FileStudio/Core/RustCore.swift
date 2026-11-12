// RustCore.swift — implements CoreAPI on top of the UniFFI bindings
// (Core/Generated/fstudio_ffi.swift, built by scripts/build-core-apple.sh).
//
// Pure translation between the generated `Ffi…` types and CoreAPI's value
// types. No decisions are made here.

import Foundation

// MARK: - Errors

private func wrap<T>(_ body: () throws -> T) throws -> T {
    do { return try body() } catch let e as FfiError {
        switch e {
        case .NotFound(let m): throw CoreError(.notFound, m)
        case .NotAvailable(let m): throw CoreError(.notAvailable, m)
        case .InvalidName(let m): throw CoreError(.invalidName, m)
        case .InvalidSetting(let m): throw CoreError(.invalidSetting, m)
        case .UnknownCommand(let m): throw CoreError(.unknownCommand, m)
        case .Failed(let m): throw CoreError(.failed, m)
        }
    }
}

// MARK: - Conversions: keys and commands

extension KeyChord {
    var ffi: FfiKeyChord {
        let k: FfiKey = switch key {
        case .char(let c): .char(value: String(c))
        case .left: .left
        case .right: .right
        case .up: .up
        case .down: .down
        case .space: .space
        case .returnKey: .return
        case .enter: .enter
        case .escape: .escape
        case .tab: .tab
        case .delete: .delete
        case .forwardDelete: .forwardDelete
        case .home: .home
        case .end: .end
        case .pageUp: .pageUp
        case .pageDown: .pageDown
        case .f(let n): .f(n: n)
        }
        return FfiKeyChord(key: k, mods: mods.ffi)
    }
}

extension Mods { var ffi: FfiMods { FfiMods(shift: shift, cmd: cmd, alt: alt, ctrl: ctrl) } }

extension InputContext {
    var ffi: FfiInputContext {
        let f: FfiFocus = switch focus {
        case .list: .list
        case .addressBar: .addressBar
        case .palette: .palette
        case .rename: .rename
        case .filter: .filter
        case .other: .other
        }
        return FfiInputContext(focus: f, tab: tab)
    }
}

extension Via {
    var ffi: FfiVia {
        switch self {
        case .keyboard: .keyboard
        case .mouse: .mouse
        case .menu: .menu
        case .palette: .palette
        case .script: .script
        }
    }
}

extension ViewMode {
    var ffi: FfiViewMode {
        switch self {
        case .icons: .icons
        case .list: .list
        case .columns: .columns
        case .gallery: .gallery
        }
    }
    init(_ f: FfiViewMode) {
        switch f {
        case .icons: self = .icons
        case .list: self = .list
        case .columns: self = .columns
        case .gallery: self = .gallery
        }
    }
}

extension CommandArg {
    var ffi: FfiCommandArg {
        switch self {
        case .path(let p): .path(path: p)
        case .index(let i): .index(index: i)
        case .viewMode(let m): .viewMode(mode: m.ffi)
        case .char(let c): .char(value: String(c))
        case .confirm: .confirm
        case .confirmed: .confirmed
        case .text(let t): .text(text: t)
        }
    }
    init?(_ f: FfiCommandArg) {
        switch f {
        case .path(let p): self = .path(p)
        case .index(let i): self = .index(i)
        case .viewMode(let m): self = .viewMode(ViewMode(m))
        case .char(let v):
            guard let c = v.first else { return nil }
            self = .char(c)
        case .confirm: self = .confirm
        case .confirmed: self = .confirmed
        case .text(let t): self = .text(t)
        }
    }
}

extension Invocation {
    var ffi: FfiInvocation { FfiInvocation(id: id, arg: arg?.ffi, tab: tab) }
    init(_ f: FfiInvocation) { self.init(id: f.id, arg: f.arg.flatMap(CommandArg.init), tab: f.tab) }
}

extension ShellRequest {
    init(_ f: FfiShellRequest) {
        switch f {
        case .open(let p): self = .open(paths: p)
        case .openWith(let p): self = .openWith(paths: p)
        case .openInEditor(let p): self = .openInEditor(paths: p)
        case .quickLook(let p): self = .quickLook(paths: p)
        case .getInfo(let p): self = .getInfo(paths: p)
        case .revealInFinder(let p): self = .revealInFinder(paths: p)
        case .openTerminal(let d): self = .openTerminal(dir: d)
        case .copyText(let t): self = .copyText(t)
        case .writeFilePasteboard(let p): self = .writeFilePasteboard(paths: p)
        case .beginRename(let t, let n): self = .beginRename(tab: t, name: n)
        case .renameDialog(let t, let p): self = .renameDialog(tab: t, paths: p)
        case .offerCreateFolder(let p, let n): self = .offerCreateFolder(path: p, nearestExisting: n)
        case .confirmTrash(let p): self = .confirmTrash(paths: p)
        case .confirmDeleteImmediately(let p): self = .confirmDeleteImmediately(paths: p)
        case .askPattern(let s): self = .askPattern(select: s)
        case .askWorkspaceName: self = .askWorkspaceName
        case .manageWorkspaces: self = .manageWorkspaces
        case .focusAddressBar(let p, let t): self = .focusAddressBar(pane: p, text: t)
        case .focusFilter: self = .focusFilter
        case .focusList: self = .focusList
        case .textField(let c): self = .textField(command: c)
        case .showPalette: self = .showPalette
        case .showQuickOpen: self = .showQuickOpen
        case .showSettings(let s): self = .showSettings(section: s)
        case .showPowerTour: self = .showPowerTour
        case .toggleSidebar: self = .toggleSidebar
        case .toggleInspector: self = .toggleInspector
        case .eject(let p): self = .eject(path: p)
        case .showDriveInfo(let p): self = .showDriveInfo(path: p)
        case .askFavouriteName(let p, let n): self = .askFavouriteName(path: p, name: n)
        case .editTags(let p): self = .editTags(paths: p)
        case .openWindow(let w): self = .openWindow(w)
        case .closeWindow(let w): self = .closeWindow(w)
        }
    }
}

extension Hint { init(_ f: FfiHint) { self.init(command: f.command, keys: f.keys, label: f.label) } }

extension Outcome {
    init(_ f: FfiOutcome) {
        self.init(command: f.command, tab: f.tab, shell: f.shell.map(ShellRequest.init), ops: f.ops,
                  message: f.message, pendingKeys: f.pendingKeys, hint: f.hint.map(Hint.init))
    }
}

// MARK: - Packed rows

/// Decodes `FfiTab::rows_packed` (little endian; layout documented in Rust).
func unpackRows(_ data: Data) -> [Row] {
    data.withUnsafeBytes { (b: UnsafeRawBufferPointer) -> [Row] in
        var at = 0
        func u32() -> UInt32 { defer { at += 4 }; return UInt32(littleEndian: b.loadUnaligned(fromByteOffset: at, as: UInt32.self)) }
        func u64() -> UInt64 { defer { at += 8 }; return UInt64(littleEndian: b.loadUnaligned(fromByteOffset: at, as: UInt64.self)) }
        func u8() -> UInt8 { defer { at += 1 }; return b[at] }
        func str() -> String {
            let n = Int(u32())
            defer { at += n }
            return String(decoding: UnsafeRawBufferPointer(rebasing: b[at..<at + n]), as: UTF8.self)
        }
        guard b.count >= 4 else { return [] }
        let n = Int(u32())
        var out: [Row] = []
        out.reserveCapacity(n)
        let kinds: [RowKind] = [.folder, .package, .file, .symlink, .other]
        for _ in 0..<n {
            let index = u32()
            let kind = kinds[min(Int(u8()), 4)]
            let f = u8()
            let size = u64()
            let modified = Int64(bitPattern: u64())
            let name = str(), icon = str(), kindText = str(), sizeText = str(), modifiedText = str()
            out.append(Row(index: index, name: name, kind: kind, iconKey: icon, kindText: kindText, size: size,
                           sizeText: sizeText, modifiedNs: modified, modifiedText: modifiedText,
                           isHidden: f & 1 != 0, isDataless: f & 2 != 0, isPackage: f & 4 != 0,
                           isSymlink: f & 8 != 0, isSelected: f & 16 != 0, isCursor: f & 32 != 0,
                           isCutPending: f & 64 != 0))
        }
        // ui-list additions: the trailer (created date, extension) per row.
        if at < b.count {
            for i in 0..<n {
                out[i].createdNs = Int64(bitPattern: u64())
                out[i].createdText = str()
                out[i].extText = str()
            }
        }
        return out
    }
}

// MARK: - Conversions: listing, view, selection, workspace

extension Row {
    init(_ r: FfiRow) {
        let k: RowKind = switch r.kind {
        case .folder: .folder
        case .package: .package
        case .file: .file
        case .symlink: .symlink
        case .other: .other
        }
        self.init(index: r.index, name: r.name, kind: k, iconKey: r.iconKey, kindText: r.kindText, size: r.size,
                  sizeText: r.sizeText, modifiedNs: r.modifiedNs, modifiedText: r.modifiedText,
                  isHidden: r.isHidden, isDataless: r.isDataless, isPackage: r.isPackage, isSymlink: r.isSymlink,
                  isSelected: r.isSelected, isCursor: r.isCursor, isCutPending: r.isCutPending)
        createdNs = r.createdNs; createdText = r.createdText; extText = r.extText
    }
}

extension ListingInfo {
    init(_ l: FfiListingInfo) {
        let s: ListingState = switch l.state {
        case .loading: .loading
        case .complete: .complete
        case .error: .error
        }
        self.init(version: l.version, dir: l.dir, state: s, error: l.error, count: l.len, totalCount: l.totalLen)
    }
}

extension ChangeKind {
    init(_ f: FfiChangeKind) {
        switch f {
        case .partial: self = .partial
        case .sorted: self = .sorted
        case .patched: self = .patched
        case .resorted: self = .resorted
        case .error: self = .error
        }
    }
}

extension SortBy {
    var ffi: FfiSortBy {
        switch self {
        case .name: .name
        case .size: .size
        case .modified: .modified
        case .created: .created
        case .kind: .kind
        case .ext: .extension
        }
    }
    init(_ f: FfiSortBy) {
        switch f {
        case .name: self = .name
        case .size: self = .size
        case .modified: self = .modified
        case .created: self = .created
        case .kind: self = .kind
        case .extension: self = .ext
        }
    }
}

extension ViewState {
    var ffi: FfiViewState {
        FfiViewState(mode: mode.ffi, sort: sort.ffi, ascending: ascending, foldersFirst: foldersFirst,
                     showHidden: showHidden, packagesAsFiles: packagesAsFiles, columns: columns.map(\.ffi))
    }
    init(_ f: FfiViewState) {
        self.init()
        mode = ViewMode(f.mode); sort = SortBy(f.sort); ascending = f.ascending; foldersFirst = f.foldersFirst
        showHidden = f.showHidden; packagesAsFiles = f.packagesAsFiles
        columns = f.columns.map(ColumnSpec.init)
    }
}

extension Selection {
    init(_ f: FfiSelection) {
        self.init()
        rows = f.rows; names = f.names; cursor = f.cursor; anchor = f.anchor; version = f.version
    }
}

extension CursorMove {
    var ffi: FfiCursorMove {
        switch self {
        case .up: .up
        case .down: .down
        case .pageUp: .pageUp
        case .pageDown: .pageDown
        case .home: .home
        case .end: .end
        }
    }
}

extension SelectionChange {
    var ffi: FfiSelectionChange {
        switch self {
        case .only(let r): .only(row: r)
        case .toggle(let r): .toggle(row: r)
        case .extendTo(let r): .extendTo(row: r)
        case .all: .all
        case .none: .none
        case .invert: .invert
        case .rows(let r): .rows(rows: r)
        case .names(let n): .names(names: n)
        case .move(let m, let e): .move(by: m.ffi, extend: e)
        }
    }
}

extension TabState {
    init(_ t: FfiTabState) {
        self.init(id: t.id, location: t.location, back: t.back, forward: t.forward, view: ViewState(t.view),
                  filter: t.filter, selected: t.selected, cursor: t.cursor,
                  scroll: t.scroll.map { ScrollAnchor(name: $0.name, offset: $0.offset) })
    }
}

extension Orientation {
    var ffi: FfiOrientation { self == .horizontal ? .horizontal : .vertical }
    init(_ f: FfiOrientation) { self = f == .horizontal ? .horizontal : .vertical }
}

extension WorkspaceState {
    init(_ w: FfiWorkspaceState) {
        self.init(
            windows: w.windows.map { win in
                WindowState(id: win.id,
                            panes: win.panes.map { PaneState(id: $0.id, tabs: $0.tabs.map(TabState.init),
                                                             activeTab: Int($0.activeTab)) },
                            activePane: Int(win.activePane), orientation: Orientation(win.orientation),
                            splitRatio: win.splitRatio)
            },
            activeWindow: w.activeWindow)
    }
}

// MARK: - Conversions: path bar

extension Crumb {
    init(_ c: FfiCrumb) {
        let k: CrumbKind = switch c.kind {
        case .volumeRoot: .volumeRoot
        case .home: .home
        case .folder: .folder
        case .package: .package
        }
        self.init(name: c.name, path: c.path, kind: k, isCurrent: c.isCurrent)
    }
}

extension ParsedLocation {
    init(_ p: FfiParsedLocation) {
        let l: Location = switch p.location {
        case .dir(let path): .dir(path)
        case .file(let d, let n): .file(dir: d, name: n)
        case .missing(let path, let n): .missing(path: path, nearestExisting: n)
        case .remote(let u): .remote(u)
        }
        self.init(location: l, cleaned: p.cleaned, chip: p.chip)
    }
}

extension Completions {
    init(_ c: FfiCompletions) {
        self.init()
        items = c.items.map {
            Completion(displayName: $0.displayName, fullPath: $0.fullPath, breadcrumbText: $0.breadcrumbText,
                       isFrecent: $0.isFrecent, matchRanges: $0.matchRanges.map { Int($0.start)..<Int($0.end) },
                       score: $0.score, insertText: $0.insertText, lastVisit: $0.lastVisit)
        }
        commonPrefixExtension = c.commonPrefixExtension.map {
            TabExtension(replaceFrom: Int($0.replaceFrom), text: $0.text, newInput: $0.newInput)
        }
        ghostText = c.ghostText; isBareWord = c.isBareWord; parent = c.parent
        segmentStart = Int(c.segmentStart); skippedSlow = c.skippedSlow
    }
}

extension PathStyle {
    var ffi: FfiPathStyle {
        switch self {
        case .posix: .posix
        case .escaped: .escaped
        case .homeRelative: .homeRelative
        case .fileURL: .fileUrl
        case .quoted: .quoted
        }
    }
}

// MARK: - Conversions: operations

extension OpKind {
    init(_ f: FfiOpKind) {
        switch f {
        case .copy: self = .copy
        case .move: self = .move
        case .trash: self = .trash
        case .deleteImmediately: self = .deleteImmediately
        case .newFolder: self = .newFolder
        case .newFile: self = .newFile
        case .rename: self = .rename
        case .duplicate: self = .duplicate
        case .moveTo: self = .moveTo
        }
    }
}

extension OpState {
    init(_ f: FfiOpState) {
        switch f {
        case .queued: self = .queued
        case .running: self = .running
        case .paused: self = .paused
        case .waitingForDecision: self = .waitingForDecision
        case .completed: self = .completed
        case .completedWithErrors: self = .completedWithErrors
        case .cancelled: self = .cancelled
        case .failed: self = .failed
        case .interrupted: self = .interrupted
        }
    }
}

extension EntryKind {
    init(_ f: FfiEntryKind) {
        switch f {
        case .file: self = .file
        case .dir: self = .dir
        case .symlink: self = .symlink
        case .other: self = .other
        }
    }
}

extension ItemError { init(_ e: FfiItemError) { self.init(path: e.path, dest: e.dest, message: e.message, errno: e.errno) } }

extension Conflict {
    init(_ c: FfiConflict) {
        self.init(op: c.op, id: c.id, source: c.source, dest: c.dest, sourceKind: EntryKind(c.sourceKind),
                  destKind: EntryKind(c.destKind), sourceSize: c.sourceSize, destSize: c.destSize,
                  sourceMtimeNs: c.sourceMtimeNs, destMtimeNs: c.destMtimeNs, canMerge: c.canMerge)
    }
}

extension OpStatus {
    init(_ s: FfiOpStatus) {
        self.init(id: s.id, kind: OpKind(s.kind), title: s.title, state: OpState(s.state), filesDone: s.filesDone,
                  filesTotal: s.filesTotal, bytesDone: s.bytesDone, bytesTotal: s.bytesTotal, current: s.current,
                  speedBps: s.speedBps, etaSecs: s.etaSecs, errors: s.errors.map(ItemError.init),
                  conflicts: s.conflicts.map(Conflict.init))
    }
}

extension OpReport {
    init(_ r: FfiOpReport) {
        self.init(id: r.id, kind: OpKind(r.kind), state: OpState(r.state), filesDone: r.filesDone,
                  filesFailed: r.filesFailed, filesSkipped: r.filesSkipped, bytesDone: r.bytesDone,
                  errors: r.errors.map(ItemError.init), elapsedMs: r.elapsedMs)
    }
}

extension UndoState { init(_ u: FfiUndoState) { self.init(undo: u.undo, redo: u.redo) } }

extension ConflictDecision {
    var ffi: FfiConflictDecision {
        switch self {
        case .keepBoth: .keepBoth
        case .replace: .replace
        case .skip: .skip
        case .merge: .merge
        }
    }
}

extension OpRequest {
    var ffi: FfiOpRequest {
        switch self {
        case .copy(let s, let d): .copy(sources: s, destDir: d)
        case .move(let s, let d): .move(sources: s, destDir: d)
        case .trash(let p): .trash(paths: p)
        case .newFolder(let p, let n): .newFolder(parent: p, name: n)
        case .newFile(let p, let n): .newFile(parent: p, name: n)
        case .rename(let p, let n): .rename(path: p, newName: n)
        case .duplicate(let p): .duplicate(paths: p)
        }
    }
}

extension Volume {
    init(_ v: FfiVolume) {
        self.init(name: v.name, mountPath: v.mountPath, isRemovable: v.isRemovable, isNetwork: v.isNetwork,
                  isReadOnly: v.isReadOnly, totalBytes: v.totalBytes, freeBytes: v.freeBytes)
    }
}

// MARK: - Conversions: menus

extension MenuTarget {
    var ffi: FfiMenuTarget {
        switch self {
        case .file(let t, let p): .file(tab: t, path: p)
        case .folder(let t, let p): .folder(tab: t, path: p)
        case .package(let t, let p): .package(tab: t, path: p)
        case .multi(let t, let p): .multi(tab: t, paths: p)
        case .emptySpace(let t): .emptySpace(tab: t)
        case .crumb(let t, let p): .crumb(tab: t, path: p)
        case .sidebarItem(let p, let k): .sidebarItem(path: p, kind: k == .favourite ? .favourite : .volume)
        case .tab(let t): .tab(tab: t)
        }
    }
    init(_ f: FfiMenuTarget) {
        switch f {
        case .file(let t, let p): self = .file(tab: t, path: p)
        case .folder(let t, let p): self = .folder(tab: t, path: p)
        case .package(let t, let p): self = .package(tab: t, path: p)
        case .multi(let t, let p): self = .multi(tab: t, paths: p)
        case .emptySpace(let t): self = .emptySpace(tab: t)
        case .crumb(let t, let p): self = .crumb(tab: t, path: p)
        case .sidebarItem(let p, let k): self = .sidebarItem(path: p, kind: k == .favourite ? .favourite : .volume)
        case .tab(let t): self = .tab(t)
        }
    }
}

extension MenuKind {
    var ffi: FfiMenuKind {
        switch self {
        case .file: .file
        case .folder: .folder
        case .package: .package
        case .multi: .multi
        case .emptySpace: .emptySpace
        case .crumb: .crumb
        case .sidebarItem: .sidebarItem
        case .tab: .tab
        }
    }
}

extension MenuItem {
    init(_ f: FfiMenuItem) {
        switch f.kind {
        case .separator: self = .separator
        case .submenu: self = .submenu(entry: f.entry, title: f.title, items: f.children.map(MenuItem.init))
        case .system:
            let k: SystemMenu = switch f.system ?? .share {
            case .openWith: .openWith
            case .share: .share
            case .quickActions: .quickActions
            case .services: .services
            }
            self = .system(k, title: f.title, paths: f.paths)
        case .command:
            self = .command(MenuCommand(entry: f.entry, title: f.title, shortcut: f.shortcut, enabled: f.enabled,
                                        checked: f.checked, tagColor: f.tagColor,
                                        invocation: f.invocation.map(Invocation.init) ?? Invocation(id: f.entry)))
        }
    }
}

// MARK: - Conversions: settings

extension Corner {
    var ffi: FfiCorner {
        switch self {
        case .topLeft: .topLeft
        case .topRight: .topRight
        case .bottomLeft: .bottomLeft
        case .bottomRight: .bottomRight
        }
    }
    init(_ f: FfiCorner) {
        switch f {
        case .topLeft: self = .topLeft
        case .topRight: self = .topRight
        case .bottomLeft: self = .bottomLeft
        case .bottomRight: self = .bottomRight
        }
    }
}

extension TerminalApp {
    var ffi: FfiTerminalApp {
        switch self {
        case .terminal: .terminal
        case .iTerm: .iTerm
        case .ghostty: .ghostty
        }
    }
    init(_ f: FfiTerminalApp) {
        switch f {
        case .terminal: self = .terminal
        case .iTerm: self = .iTerm
        case .ghostty: self = .ghostty
        }
    }
}

extension RowDensity {
    var ffi: FfiRowDensity {
        switch self {
        case .compact: .compact
        case .regular: .regular
        case .comfortable: .comfortable
        }
    }
    init(_ f: FfiRowDensity) {
        switch f {
        case .compact: self = .compact
        case .regular: self = .regular
        case .comfortable: self = .comfortable
        }
    }
}

extension HintFrequency {
    var ffi: FfiHintFrequency {
        switch self {
        case .always: .always
        case .sometimes: .sometimes
        case .rarely: .rarely
        }
    }
    init(_ f: FfiHintFrequency) {
        switch f {
        case .always: self = .always
        case .sometimes: self = .sometimes
        case .rarely: self = .rarely
        }
    }
}

extension Handling {
    init(_ f: FfiHandling) {
        switch f {
        case .core: self = .core
        case .shell: self = .shell
        case .later: self = .later
        }
    }
}

extension Settings {
    init(_ s: FfiSettings) {
        self.init()
        let g = s.general
        general = General(restoreSession: g.restoreSession, startFolder: g.startFolder,
                          dualPaneByDefault: g.dualPaneByDefault, confirmTrash: g.confirmTrash,
                          typeSelectTimeoutMs: g.typeSelectTimeoutMs, closedTabsKept: g.closedTabsKept,
                          terminalApp: TerminalApp(g.terminalApp))
        let v = s.view
        view = View(mode: ViewMode(v.mode), sort: SortBy(v.sort), ascending: v.ascending,
                    foldersFirst: v.foldersFirst, showHidden: v.showHidden, packagesAsFiles: v.packagesAsFiles,
                    folderMemory: v.folderMemory, hour24: v.hour24,
                    density: RowDensity(v.density), columns: v.columns.map(ColumnSpec.init))
        pathBar = PathBar(completionLimit: s.pathBar.completionLimit, completeHidden: s.pathBar.completeHidden,
                          frecencyExcluded: s.pathBar.frecencyExcluded, maxCrumbs: s.pathBar.maxCrumbs)
        let o = s.operations
        operations = Operations(verifyCopies: o.verifyCopies, fsyncCopies: o.fsyncCopies,
                                allowDeleteImmediately: o.allowDeleteImmediately, fileWorkers: o.fileWorkers,
                                opsPerVolume: o.opsPerVolume, undoLimit: o.undoLimit)
        keyboard = Keyboard(profile: s.keyboard.profile == .vim ? .vim : .standard,
                            sequenceTimeoutMs: s.keyboard.sequenceTimeoutMs,
                            typingFilters: s.keyboard.typingFilters, fnKeysAlways: s.keyboard.fnKeysAlways)
        overlay = Overlay(enabled: s.overlay.enabled, delayMs: s.overlay.delayMs, corner: Corner(s.overlay.corner),
                          listPlainKeys: s.overlay.listPlainKeys)
        hints = Hints(enabled: s.hints.enabled, frequency: HintFrequency(s.hints.frequency),
                      seconds: s.hints.seconds, corner: Corner(s.hints.corner),
                      stopWhenLearned: s.hints.stopWhenLearned)
    }

    var ffi: FfiSettings {
        FfiSettings(
            general: FfiGeneralSettings(
                restoreSession: general.restoreSession, startFolder: general.startFolder,
                dualPaneByDefault: general.dualPaneByDefault, confirmTrash: general.confirmTrash,
                typeSelectTimeoutMs: general.typeSelectTimeoutMs, closedTabsKept: general.closedTabsKept,
                terminalApp: general.terminalApp.ffi),
            view: FfiViewSettings(
                mode: view.mode.ffi, sort: view.sort.ffi, ascending: view.ascending, foldersFirst: view.foldersFirst,
                showHidden: view.showHidden, packagesAsFiles: view.packagesAsFiles, folderMemory: view.folderMemory,
                hour24: view.hour24, density: view.density.ffi, columns: view.columns.map(\.ffi)),
            pathBar: FfiPathBarSettings(completionLimit: pathBar.completionLimit, completeHidden: pathBar.completeHidden,
                                        frecencyExcluded: pathBar.frecencyExcluded, maxCrumbs: pathBar.maxCrumbs),
            operations: FfiOperationsSettings(
                verifyCopies: operations.verifyCopies, fsyncCopies: operations.fsyncCopies,
                allowDeleteImmediately: operations.allowDeleteImmediately, fileWorkers: operations.fileWorkers,
                opsPerVolume: operations.opsPerVolume, undoLimit: operations.undoLimit),
            keyboard: FfiKeyboardSettings(profile: keyboard.profile == .vim ? .vim : .standard,
                                          sequenceTimeoutMs: keyboard.sequenceTimeoutMs,
                                          typingFilters: keyboard.typingFilters, fnKeysAlways: keyboard.fnKeysAlways),
            overlay: FfiOverlaySettings(enabled: overlay.enabled, delayMs: overlay.delayMs, corner: overlay.corner.ffi,
                                        listPlainKeys: overlay.listPlainKeys),
            hints: FfiHintSettings(enabled: hints.enabled, frequency: hints.frequency.ffi,
                                   seconds: hints.seconds, corner: hints.corner.ffi,
                                   stopWhenLearned: hints.stopWhenLearned))
    }
}

// MARK: - Events

func coreEvent(_ e: FfiEvent) -> CoreEvent {
    switch e {
    case .listingChanged(let t, let v, let k): .listingChanged(tab: t, version: v, kind: ChangeKind(k))
    case .selectionChanged(let t, let n, let c): .selectionChanged(tab: t, count: n, cursor: c)
    case .tabsChanged: .tabsChanged
    case .opProgress(let ops): .opProgress(ops.map(OpStatus.init))
    case .opConflict(let op, let c): .opConflict(op: op, conflict: Conflict(c))
    case .opFailed(let op, let p, let err): .opFailed(op: op, path: p, error: err)
    case .opFinished(let op, let r): .opFinished(op: op, report: OpReport(r))
    case .undoChanged(let u): .undoChanged(UndoState(u))
    case .volumesChanged(let v): .volumesChanged(v.map(Volume.init))
    case .hint(let h): .hint(Hint(h))
    case .settingsChanged: .settingsChanged
    case .favouritesChanged: .favouritesChanged
    }
}

private final class SinkBridge: FfiEventSink, @unchecked Sendable {
    private let lock = NSLock()
    private var handler: (@Sendable (CoreEvent) -> Void)?
    func set(_ h: @escaping @Sendable (CoreEvent) -> Void) { lock.withLock { handler = h } }
    func onEvent(event: FfiEvent) {
        let h = lock.withLock { handler }
        h?(coreEvent(event))
    }
}

// MARK: - Tab

final class RustTab: TabAPI, @unchecked Sendable {
    let ffi: FfiTab
    let id: TabID
    init(_ t: FfiTab) { ffi = t; id = t.id() }

    func location() -> String { ffi.location() }
    func state() -> TabState { TabState(ffi.state()) }
    func listingInfo() -> ListingInfo { ListingInfo(ffi.listingInfo()) }
    /// Uses the packed path: the generated record lifting costs ~10× the
    /// Rust side for 60 rows (see `FfiTab::rows_packed` for the layout).
    func rows(start: UInt32, count: UInt32) -> [Row] { unpackRows(ffi.rowsPacked(start: start, count: count)) }
    func indexOf(name: String) -> UInt32? { ffi.indexOf(name: name) }
    func pathAt(_ row: UInt32) -> String? { ffi.pathAt(i: row) }
    func go(_ to: String) throws -> Outcome { try wrap { Outcome(try ffi.go(to: to)) } }
    func back() throws -> Outcome { try wrap { Outcome(try ffi.back()) } }
    func forward() throws -> Outcome { try wrap { Outcome(try ffi.forward()) } }
    func up() throws -> Outcome { try wrap { Outcome(try ffi.up()) } }
    var canGoBack: Bool { ffi.canGoBack() }
    var canGoForward: Bool { ffi.canGoForward() }
    func refresh() { ffi.refresh() }
    func view() -> ViewState { ViewState(ffi.view()) }
    func setView(_ v: ViewState) { ffi.setView(view: v.ffi) }
    func filter() -> String { ffi.filter() }
    func setFilter(_ text: String) { ffi.setFilter(text: text) }
    func selection() -> Selection { Selection(ffi.selection()) }
    func selectedPaths() -> [String] { ffi.selectedPaths() }
    func setSelection(_ change: SelectionChange) { ffi.setSelection(change: change.ffi) }
    func selectPattern(_ pattern: String, select: Bool) { ffi.selectPattern(pattern: pattern, select: select) }
    func typeSelect(_ text: String) -> UInt32? { ffi.typeSelect(text: text) }
    func setVisibleRange(start: UInt32, count: UInt32) { ffi.setVisibleRange(start: start, count: count) }
    func setScrollAnchor(_ a: ScrollAnchor?) {
        ffi.setScrollAnchor(anchor: a.map { FfiScrollAnchor(name: $0.name, offset: $0.offset) })
    }
    func scrollAnchor() -> ScrollAnchor? { ffi.scrollAnchor().map { ScrollAnchor(name: $0.name, offset: $0.offset) } }
    func breadcrumbs() -> [Crumb] { ffi.breadcrumbs().map(Crumb.init) }
    func complete(_ input: String, cursor: UInt32) -> Completions { Completions(ffi.complete(input: input, cursor: cursor)) }
}

// MARK: - Engine

final class RustEngine: EngineAPI, @unchecked Sendable {
    private let ffi: FfiEngine
    private let sink = SinkBridge()
    let implementationName = "Rust"

    /// `dataDir`: the database folder (`~/Library/Application Support/File Studio`).
    init(dataDir: URL) throws {
        try FileManager.default.createDirectory(at: dataDir, withIntermediateDirectories: true)
        let sink = self.sink
        ffi = try wrap {
            try FfiEngine(config: FfiEngineConfig(dataDir: dataDir.path, home: nil, trashDir: nil, pollWatcher: false),
                          sink: sink)
        }
    }

    func setEventHandler(_ handler: @escaping @Sendable (CoreEvent) -> Void) { sink.set(handler) }
    func home() -> String { ffi.home() }

    // Settings
    func settings() -> Settings { Settings(ffi.settings()) }
    func updateSettings(_ s: Settings) throws { try wrap { try ffi.updateSettings(s: s.ffi) } }
    func setting(_ key: String) throws -> String { try wrap { try ffi.setting(key: key) } }
    func setSetting(_ key: String, json: String) throws { try wrap { try ffi.setSetting(key: key, value: json) } }

    // Workspace
    func restoreSession() -> WorkspaceState { WorkspaceState(ffi.restoreSession()) }
    func workspaceState() -> WorkspaceState { WorkspaceState(ffi.workspaceState()) }
    func tab(_ id: TabID) -> TabAPI? { ffi.tab(id: id).map(RustTab.init) }
    func activeTab() -> TabAPI? { ffi.activeTab().map(RustTab.init) }
    func otherPaneTab(of: TabID) -> TabAPI? { ffi.otherPaneTab(of: of).map(RustTab.init) }
    func newWindow(path: String?) throws -> WindowID { try wrap { try ffi.newWindow(path: path) } }
    func closeWindow(_ id: WindowID) throws { try wrap { try ffi.closeWindow(id: id) } }
    func openTab(path: String?) throws -> TabAPI { try wrap { RustTab(try ffi.openTab(path: path)) } }
    func openTab(inPane pane: PaneID, path: String?, index: UInt32?) throws -> TabAPI {
        try wrap { RustTab(try ffi.openTabIn(pane: pane, path: path, index: index)) }
    }
    func closeTab(_ id: TabID) throws { try wrap { try ffi.closeTab(id: id) } }
    func closeOtherTabs(keep: TabID) throws { try wrap { try ffi.closeOtherTabs(keep: keep) } }
    func reopenClosedTab() throws -> TabAPI? { try wrap { try ffi.reopenClosedTab().map(RustTab.init) } }
    func activateTab(_ id: TabID) throws { try wrap { try ffi.activateTab(id: id) } }
    func moveTab(_ id: TabID, toPane pane: PaneID, index: UInt32) throws {
        try wrap { try ffi.moveTab(id: id, pane: pane, index: index) }
    }
    func focusPane(window: WindowID, index: UInt32) throws { try wrap { try ffi.focusPane(window: window, index: index) } }
    func toggleDual(window: WindowID) throws -> Bool { try wrap { try ffi.toggleDual(window: window) } }
    func setLayout(window: WindowID, orientation: Orientation, splitRatio: Double) throws {
        try wrap { try ffi.setLayout(window: window, orientation: orientation.ffi, splitRatio: splitRatio) }
    }
    func swapPanes(window: WindowID) throws { try wrap { try ffi.swapPanes(window: window) } }
    func saveWorkspace(name: String) throws -> WorkspaceID { try wrap { try ffi.saveWorkspace(name: name) } }
    func savedWorkspaces() throws -> [WorkspaceSummary] {
        try wrap {
            try ffi.savedWorkspaces().map {
                WorkspaceSummary(id: $0.id, name: $0.name, windows: $0.windows, tabs: $0.tabs, updatedNs: $0.updatedNs)
            }
        }
    }
    func openWorkspace(_ id: WorkspaceID) throws -> WorkspaceState {
        try wrap { WorkspaceState(try ffi.openWorkspace(id: id)) }
    }
    func renameWorkspace(_ id: WorkspaceID, name: String) throws { try wrap { try ffi.renameWorkspace(id: id, name: name) } }
    func deleteWorkspace(_ id: WorkspaceID) throws { try wrap { try ffi.deleteWorkspace(id: id) } }

    // Commands
    func handleKey(_ chord: KeyChord, context: InputContext) -> Outcome? {
        ffi.handleKey(chord: chord.ffi, ctx: context.ffi).map(Outcome.init)
    }
    func pollKeys(context: InputContext) -> Outcome? { ffi.pollKeys(ctx: context.ffi).map(Outcome.init) }
    func pendingKeys() -> String? { ffi.pendingKeys() }
    func perform(_ inv: Invocation, via: Via) throws -> Outcome {
        try wrap { Outcome(try ffi.perform(inv: inv.ffi, via: via.ffi)) }
    }
    func commands() -> [CommandInfo] {
        ffi.commands().map {
            CommandInfo(id: $0.id, title: $0.title, category: $0.category, description: $0.description,
                        shortcut: $0.shortcut, showsInMenu: $0.showsInMenu, showsInPalette: $0.showsInPalette,
                        customized: $0.customized, handling: Handling($0.handling))
        }
    }
    func heldShortcuts(_ mods: Mods, context: InputContext) -> HeldShortcuts? {
        ffi.heldShortcuts(mods: mods.ffi, ctx: context.ffi).map {
            HeldShortcuts(title: $0.title, rows: $0.rows.map {
                ShortcutRow(command: $0.command, keys: $0.keys, label: $0.label, category: $0.category)
            })
        }
    }
    func hintFor(command: String, via: Via) -> Hint? { ffi.hintFor(command: command, via: via.ffi).map(Hint.init) }
    func vimMode() -> VimMode {
        switch ffi.vimMode() {
        case .off: .off
        case .normal: .normal
        case .visual: .visual
        }
    }
    func commandKeys(_ command: String) -> [String] { ffi.commandKeys(command: command) }
    func setCommandKey(_ command: String, primary: Bool, keys: String?) throws {
        try wrap { try ffi.setCommandKey(command: command, primary: primary, keys: keys) }
    }
    func resetCommandKeys(_ command: String?) throws { try wrap { try ffi.resetCommandKeys(command: command) } }
    func keymapConflicts() -> [String] { ffi.keymapConflicts() }
    func renameItem(tab: TabID, oldName: String, newName: String) throws -> Outcome {
        try wrap { Outcome(try ffi.renameItem(tab: tab, oldName: oldName, newName: newName)) }
    }
    func createFolderAndGo(tab: TabID, path: String) throws -> Outcome {
        try wrap { Outcome(try ffi.createFolderAndGo(tab: tab, path: path)) }
    }
    func setClipboard(_ paths: [String], cut: Bool) { ffi.setClipboard(paths: paths, cut: cut) }
    func dropAction(sources: [String], destDir: String, mods: Mods) -> DropAction {
        switch ffi.dropAction(sources: sources, destDir: destDir, mods: mods.ffi) {
        case .none: .none
        case .copy: .copy
        case .move: .move
        }
    }
    func dropFiles(sources: [String], destDir: String, mods: Mods) throws -> Outcome {
        try wrap { Outcome(try ffi.dropFiles(sources: sources, destDir: destDir, mods: mods.ffi)) }
    }
    func clipboardPaths() -> [String] { ffi.clipboardPaths() }

    // Menus
    func menuTarget(tab: TabID, row: UInt32?) throws -> MenuTarget {
        try wrap { MenuTarget(try ffi.menuTargetAt(tab: tab, row: row)) }
    }
    func contextMenu(_ target: MenuTarget, context: MenuContext) throws -> [MenuItem] {
        try wrap {
            try ffi.contextMenu(target: target.ffi, ctx: FfiMenuContext(pasteboardHasFiles: context.pasteboardHasFiles))
                .map(MenuItem.init)
        }
    }
    func menuEntries(_ kind: MenuKind) -> [MenuEntry] {
        ffi.menuEntries(kind: kind.ffi).map { MenuEntry(entry: $0.entry, title: $0.title) }
    }
    func menuChoices(_ kind: MenuKind) -> [MenuEntry] {
        ffi.menuChoices(kind: kind.ffi).map { MenuEntry(entry: $0.entry, title: $0.title) }
    }
    func menuAdd(_ kind: MenuKind, entry: String, after: String?) throws {
        try wrap { try ffi.menuAdd(kind: kind.ffi, entry: entry, after: after) }
    }
    func menuRemove(_ kind: MenuKind, entry: String) throws { try wrap { try ffi.menuRemove(kind: kind.ffi, entry: entry) } }
    func menuMove(_ kind: MenuKind, entry: String, after: String?) throws {
        try wrap { try ffi.menuMove(kind: kind.ffi, entry: entry, after: after) }
    }
    func resetMenu(_ kind: MenuKind) throws { try wrap { try ffi.resetMenu(kind: kind.ffi) } }
    func resetMenus() throws { try wrap { try ffi.resetMenus() } }

    // Path bar
    func parseLocation(_ raw: String, cwd: String) -> ParsedLocation { ParsedLocation(ffi.parseLocation(raw: raw, cwd: cwd)) }
    func breadcrumbs(path: String) -> [Crumb] { ffi.breadcrumbs(path: path).map(Crumb.init) }
    func subfolders(path: String) -> [Crumb] { ffi.subfolders(path: path).map(Crumb.init) }
    func complete(_ input: String, cursor: UInt32, cwd: String) -> Completions {
        Completions(ffi.complete(input: input, cursor: cursor, cwd: cwd))
    }
    func copyPath(_ path: String, style: PathStyle) -> String { ffi.copyPathAs(path: path, style: style.ffi) }
    func importZoxide(dbPath: String?) throws -> (added: UInt32, merged: UInt32) {
        let r = try wrap { try ffi.importZoxide(dbPath: dbPath) }
        return (r.first ?? 0, r.count > 1 ? r[1] : 0)
    }
    func makeTabCycle() -> TabCycleAPI { RustTabCycle() }
    func deletePathComponent(_ input: String, cursor: Int) -> (text: String, cursor: Int) {
        let e = FileStudio.deletePathComponent(input: input, cursor: UInt32(cursor))
        return (e.text, Int(e.cursor))
    }
    func frecent(_ query: String, limit: UInt32) -> [FrecentDir] {
        ffi.frecent(query: query, limit: limit).map { FrecentDir(path: $0.path, score: $0.score, lastAccessed: $0.lastAccessed) }
    }

    // Operations
    func enqueue(_ req: OpRequest) throws -> OpID { try wrap { try ffi.enqueue(req: req.ffi) } }
    func opsList() -> [OpStatus] { ffi.opsList().map(OpStatus.init) }
    func opStatus(_ id: OpID) -> OpStatus? { ffi.opStatus(id: id).map(OpStatus.init) }
    func pauseOp(_ id: OpID) throws { try wrap { try ffi.opPause(id: id) } }
    func resumeOp(_ id: OpID) throws { try wrap { try ffi.opResume(id: id) } }
    func cancelOp(_ id: OpID) throws { try wrap { try ffi.opCancel(id: id) } }
    func reorderOp(_ id: OpID, to index: UInt32) throws { try wrap { try ffi.opReorder(id: id, newIndex: index) } }
    func resolveConflict(op: OpID, conflict: ConflictID, decision: ConflictDecision, applyToAll: Bool) throws {
        try wrap { try ffi.resolveConflict(op: op, conflict: conflict, decision: decision.ffi, applyToAll: applyToAll) }
    }
    func interruptedOps() throws -> [OpStatus] { try wrap { try ffi.interruptedOps().map(OpStatus.init) } }
    func clearFinishedOps() { ffi.clearFinishedOps() }
    func undo() throws -> Outcome { try wrap { Outcome(try ffi.undo()) } }
    func redo() throws -> Outcome { try wrap { Outcome(try ffi.redo()) } }
    func undoState() -> UndoState { UndoState(ffi.undoState()) }

    // Sidebar
    func volumes() -> [Volume] { ffi.volumes().map(Volume.init) }
    func favourites() -> [Favourite] { ffi.favourites().map { Favourite(path: $0.path, name: $0.name, renamed: $0.renamed, missing: $0.missing) } }
    func addFavourite(_ path: String, index: UInt32?) throws { try wrap { try ffi.addFavourite(path: path, index: index) } }
    func removeFavourite(_ path: String) throws { try wrap { try ffi.removeFavourite(path: path) } }
    func renameFavourite(_ path: String, name: String) throws { try wrap { try ffi.renameFavourite(path: path, name: name) } }
    func moveFavourite(_ path: String, to index: UInt32) throws { try wrap { try ffi.moveFavourite(path: path, index: index) } }

    // OS hooks
    func volumesChanged() { ffi.volumesChanged() }
    func appWillQuit() { ffi.appWillQuit() }
}

/// The core's Tab state machine (`FfiTabCycle`).
final class RustTabCycle: TabCycleAPI {
    private let ffi = FfiTabCycle()
    func press(_ c: Completions, backwards: Bool) -> TabOutcome {
        switch ffi.press(rows: c.items.map(\.insertText), extension: c.commonPrefixExtension?.newInput,
                         backwards: backwards) {
        case .nothing: .nothing
        case .extend(let t): .extend(newInput: t)
        case .select(let i, let t): .select(index: Int(i), text: t)
        }
    }
    func reset() { ffi.reset() }
    var isCycling: Bool { ffi.isCycling() }
}

// MARK: - ui-list additions

extension ColumnSpec {
    init(_ f: FfiColumnSpec) {
        let id: ColumnID = switch f.id {
        case .name: .name
        case .size: .size
        case .modified: .modified
        case .created: .created
        case .kind: .kind
        case .extension: .ext
        }
        self.init(id: id, width: f.width)
    }
    var ffi: FfiColumnSpec {
        let i: FfiColumnId = switch id {
        case .name: .name
        case .size: .size
        case .modified: .modified
        case .created: .created
        case .kind: .kind
        case .ext: .extension
        }
        return FfiColumnSpec(id: i, width: width)
    }
}

/// A tab-less listing (column view) over `FfiListing`.
final class RustListing: ListingAPI, @unchecked Sendable {
    let ffi: FfiListing
    let id: TabID
    init(_ l: FfiListing) { ffi = l; id = l.id() }
    func dir() -> String { ffi.dir() }
    func listingInfo() -> ListingInfo { ListingInfo(ffi.listingInfo()) }
    func rows(start: UInt32, count: UInt32) -> [Row] { unpackRows(ffi.rowsPacked(start: start, count: count)) }
    func indexOf(name: String) -> UInt32? { ffi.indexOf(name: name) }
    func pathAt(_ row: UInt32) -> String? { ffi.pathAt(i: row) }
    func selection() -> Selection { Selection(ffi.selection()) }
    func setSelection(_ change: SelectionChange) { ffi.setSelection(change: change.ffi) }
    func selectedPaths() -> [String] { ffi.selectedPaths() }
    func setVisibleRange(start: UInt32, count: UInt32) { ffi.setVisibleRange(start: start, count: count) }
    func refresh() { ffi.refresh() }
    func menuTarget(owner: TabID, row: UInt32?) -> MenuTarget { MenuTarget(ffi.menuTargetAt(owner: owner, row: row)) }
}

extension RustEngine: ListEngineAPI {
    func openListing(path: String, showHidden: Bool) -> ListingAPI {
        RustListing(ffi.openListing(path: path, showHidden: showHidden))
    }
    func columnSelect(tab: TabID, dir: String, name: String?) throws -> Outcome {
        try wrap { Outcome(try ffi.columnSelect(tab: tab, dir: dir, name: name)) }
    }
    func dropEffect(sources: [String], dest: String, mods: Mods) -> DropEffect {
        switch ffi.dropEffect(sources: sources, dest: dest, mods: mods.ffi) {
        case .copy: .copy
        case .move: .move
        case .none: .none
        }
    }
    func dropItems(sources: [String], dest: String, mods: Mods) throws -> OpID {
        try wrap { try ffi.dropItems(sources: sources, dest: dest, mods: mods.ffi) }
    }
    func sortByColumn(tab: TabID, column: ColumnID) throws -> Outcome {
        try wrap { Outcome(try ffi.sortByColumn(tab: tab, column: ColumnSpec(id: column, width: 0).ffi.id)) }
    }
}

// CoreAPI.swift — the ONLY interface the Mac UI uses to talk to the core.
//
// Contract between the views and the Rust core (F-001, Import Studio's D-019):
// - `RustCore.swift` implements these protocols over the UniFFI bindings
//   (Core/Generated/fstudio_ffi.swift, built by scripts/build-core-apple.sh).
// - `Mock/MockCore.swift` implements them with synthetic data, so the UI can
//   be built and run with no Rust (`-FSMockCore`).
// - `App/EngineFactory.swift` is the only place that picks one.
//
// Rule: no app logic in Swift. Views draw what the core says (rows, crumbs,
// menus, completions), turn input into `KeyChord`s, `Invocation`s and
// `SelectionChange`s, and carry out `ShellRequest`s (Cocoa-only work). What
// a key means, what a menu contains, what selection follows a delete, how
// rows are sorted and formatted: all core.
//
// Threads: every call may be made from any thread and returns quickly unless
// its doc says it touches the disk. In practice the app calls from the main
// actor. Events arrive on a core thread; `AppModel` hops them to the main actor.
//
// Paths are plain `String`s (POSIX, exactly as the filesystem spells them).

import Foundation

public typealias TabID = UInt64
public typealias PaneID = UInt64
public typealias WindowID = UInt64
public typealias WorkspaceID = UInt64
public typealias OpID = Int64
public typealias ConflictID = Int64

// MARK: - Errors

/// Why a core call failed. `message` is plain text for the user.
public struct CoreError: Error, Equatable, Sendable, LocalizedError {
    public enum Kind: Equatable, Sendable {
        /// No tab, window, item… with that id or path.
        case notFound
        /// Can't run right now (nothing selected, single pane…). Usually a beep.
        case notAvailable
        /// A typed name can't be used.
        case invalidName
        /// A setting value was refused.
        case invalidSetting
        /// The command id isn't registered.
        case unknownCommand
        /// Database, I/O, queue.
        case failed
    }
    public var kind: Kind
    public var message: String
    public init(_ kind: Kind, _ message: String) { self.kind = kind; self.message = message }
    public var errorDescription: String? { message }
}

// MARK: - Keys and commands

/// A key press in neutral form (the core's `KeyChord`). Build it from an
/// `NSEvent` with `KeyChord(event:)` (App/KeyChord+NSEvent.swift).
public struct KeyChord: Equatable, Hashable, Sendable {
    public enum Key: Equatable, Hashable, Sendable {
        /// A printable character as typed *without* Shift, lowercased.
        case char(Character)
        case left, right, up, down, space, returnKey, enter, escape, tab
        /// ⌫
        case delete
        /// ⌦
        case forwardDelete
        case home, end, pageUp, pageDown
        /// F1–F20.
        case f(UInt8)
    }
    public var key: Key
    public var mods: Mods
    public init(key: Key, mods: Mods = Mods()) { self.key = key; self.mods = mods }
}

/// Modifier keys.
public struct Mods: Equatable, Hashable, Sendable {
    public var shift = false, cmd = false, alt = false, ctrl = false
    public init(shift: Bool = false, cmd: Bool = false, alt: Bool = false, ctrl: Bool = false) {
        self.shift = shift; self.cmd = cmd; self.alt = alt; self.ctrl = ctrl
    }
    public var isEmpty: Bool { !shift && !cmd && !alt && !ctrl }
}

/// Which part of the UI has keyboard focus when a key is pressed.
public enum Focus: Equatable, Sendable { case list, addressBar, palette, rename, filter, other }

/// Where a key press happened.
public struct InputContext: Equatable, Sendable {
    public var focus: Focus
    /// The tab whose pane has focus (nil: the focused tab).
    public var tab: TabID?
    public init(focus: Focus, tab: TabID? = nil) { self.focus = focus; self.tab = tab }
}

/// How a command was issued (hints count mouse uses; keyboard uses teach).
public enum Via: Equatable, Sendable { case keyboard, mouse, menu, palette, script }

public enum ViewMode: Equatable, Hashable, Sendable, CaseIterable { case icons, list, columns, gallery }

/// A command's argument.
public enum CommandArg: Equatable, Hashable, Sendable {
    case path(String)
    case index(UInt32)
    case viewMode(ViewMode)
    case char(Character)
    /// Ask before doing it.
    case confirm
    /// The user said yes to the core's question (`confirmTrash`…): do it now.
    case confirmed
    case text(String)
}

/// A command to run: `engine.perform(Invocation(id: "file.new_folder"), via: .menu)`.
public struct Invocation: Equatable, Hashable, Sendable {
    public var id: String
    public var arg: CommandArg?
    /// The tab it acts on (nil: the focused tab).
    public var tab: TabID?
    public init(id: String, arg: CommandArg? = nil, tab: TabID? = nil) { self.id = id; self.arg = arg; self.tab = tab }
}

/// Who carries a command out.
public enum Handling: Equatable, Sendable { case core, shell, later }

/// A registered command (for menus, the palette and Settings).
public struct CommandInfo: Equatable, Sendable, Identifiable {
    public var id: String
    public var title: String
    public var category: String
    public var description: String
    /// The primary shortcut as key-cap text (`⇧⌘N`).
    public var shortcut: String?
    public var showsInMenu: Bool
    public var showsInPalette: Bool
    public var customized: Bool
    public var handling: Handling
    public init(id: String, title: String, category: String, description: String, shortcut: String?,
                showsInMenu: Bool, showsInPalette: Bool, customized: Bool, handling: Handling) {
        self.id = id; self.title = title; self.category = category; self.description = description
        self.shortcut = shortcut; self.showsInMenu = showsInMenu; self.showsInPalette = showsInPalette
        self.customized = customized; self.handling = handling
    }
}

/// A "Did you know?" hint.
public struct Hint: Equatable, Sendable, Identifiable {
    public var command: String
    public var keys: String
    public var label: String
    public var id: String { command }
    public init(command: String, keys: String, label: String) { self.command = command; self.keys = keys; self.label = label }
}

/// One row of the held-modifier overlay.
public struct ShortcutRow: Equatable, Sendable, Identifiable {
    public var command: String
    /// Key caps without modifiers.
    public var keys: String
    public var label: String
    public var category: String
    public var id: String { command + keys }
    public init(command: String, keys: String, label: String, category: String) {
        self.command = command; self.keys = keys; self.label = label; self.category = category
    }
}

/// What the overlay shows while modifiers are held.
public struct HeldShortcuts: Equatable, Sendable {
    /// Badge text (`⌘ Command`).
    public var title: String
    public var rows: [ShortcutRow]
    public init(title: String, rows: [ShortcutRow]) { self.title = title; self.rows = rows }
}

public enum VimMode: Equatable, Sendable { case off, normal, visual }

// MARK: - Outcomes and shell requests

/// Work only the shell can do. The core already decided what the input
/// means; the shell carries it out (usually in `Platform/`).
public enum ShellRequest: Equatable, Sendable {
    case open(paths: [String])
    case openWith(paths: [String])
    case openInEditor(paths: [String])
    case quickLook(paths: [String])
    case getInfo(paths: [String])
    case revealInFinder(paths: [String])
    case openTerminal(dir: String)
    case copyText(String)
    case writeFilePasteboard(paths: [String])
    /// Start editing a row's name in place; commit with `engine.renameItem`.
    case beginRename(tab: TabID, name: String)
    case renameDialog(tab: TabID, paths: [String])
    /// Offer "Create Folder"; on yes call `engine.createFolderAndGo`.
    case offerCreateFolder(path: String, nearestExisting: String)
    /// On yes perform `file.trash` with `.confirmed`.
    case confirmTrash(paths: [String])
    case confirmDeleteImmediately(paths: [String])
    case askPattern(select: Bool)
    case askWorkspaceName
    case manageWorkspaces
    case focusAddressBar(pane: PaneID, text: String)
    case focusFilter
    case focusList
    /// A text-editing command for the focused field (`addressbar.complete_next`…).
    case textField(command: String)
    case showPalette
    case showQuickOpen
    case showSettings(section: String)
    case toggleSidebar
    case toggleInspector
    case eject(path: String)
    case showDriveInfo(path: String)
    /// Ask for a favourite's new name, then `engine.renameFavourite`.
    case askFavouriteName(path: String, name: String)
    case editTags(paths: [String])
    case openWindow(WindowID)
    case closeWindow(WindowID)

    /// A stable name per case, for handler registration (`AppModel.handle(_:)`).
    public var kind: String {
        switch self {
        case .open: "open"
        case .openWith: "openWith"
        case .openInEditor: "openInEditor"
        case .quickLook: "quickLook"
        case .getInfo: "getInfo"
        case .revealInFinder: "revealInFinder"
        case .openTerminal: "openTerminal"
        case .copyText: "copyText"
        case .writeFilePasteboard: "writeFilePasteboard"
        case .beginRename: "beginRename"
        case .renameDialog: "renameDialog"
        case .offerCreateFolder: "offerCreateFolder"
        case .confirmTrash: "confirmTrash"
        case .confirmDeleteImmediately: "confirmDeleteImmediately"
        case .askPattern: "askPattern"
        case .askWorkspaceName: "askWorkspaceName"
        case .manageWorkspaces: "manageWorkspaces"
        case .focusAddressBar: "focusAddressBar"
        case .focusFilter: "focusFilter"
        case .focusList: "focusList"
        case .textField: "textField"
        case .showPalette: "showPalette"
        case .showQuickOpen: "showQuickOpen"
        case .showSettings: "showSettings"
        case .toggleSidebar: "toggleSidebar"
        case .toggleInspector: "toggleInspector"
        case .eject: "eject"
        case .showDriveInfo: "showDriveInfo"
        case .askFavouriteName: "askFavouriteName"
        case .editTags: "editTags"
        case .openWindow: "openWindow"
        case .closeWindow: "closeWindow"
        }
    }
}

/// What a command, key press or navigation did.
public struct Outcome: Equatable, Sendable {
    public var command: String?
    public var tab: TabID?
    public var shell: ShellRequest?
    /// Operations queued (follow them with `.opProgress` / `.opFinished`).
    public var ops: [OpID] = []
    /// A short status message to show ("Copied path").
    public var message: String?
    /// An unfinished key sequence to show (`g`).
    public var pendingKeys: String?
    public var hint: Hint?
    public init(command: String? = nil, tab: TabID? = nil, shell: ShellRequest? = nil, ops: [OpID] = [],
                message: String? = nil, pendingKeys: String? = nil, hint: Hint? = nil) {
        self.command = command; self.tab = tab; self.shell = shell; self.ops = ops
        self.message = message; self.pendingKeys = pendingKeys; self.hint = hint
    }
}

// MARK: - Listing

public enum RowKind: Equatable, Sendable { case folder, package, file, symlink, other }

/// One row, ready to draw. Names exactly as on disk; strings pre-formatted.
public struct Row: Equatable, Sendable, Identifiable {
    public var index: UInt32
    public var name: String
    public var kind: RowKind
    /// `folder`, `package.app`, `file.png`, `file`, `symlink`, `other` →
    /// `IconCache` (Platform/).
    public var iconKey: String
    public var kindText: String
    public var size: UInt64
    public var sizeText: String
    public var modifiedNs: Int64
    public var modifiedText: String
    public var isHidden: Bool
    /// An iCloud placeholder: never read its content (F-016).
    public var isDataless: Bool
    public var isPackage: Bool
    public var isSymlink: Bool
    public var isSelected: Bool
    public var isCursor: Bool
    /// Cut, waiting for Paste: draw dimmed.
    public var isCutPending: Bool
    public var id: String { name }
    public init(index: UInt32, name: String, kind: RowKind, iconKey: String, kindText: String, size: UInt64,
                sizeText: String, modifiedNs: Int64, modifiedText: String, isHidden: Bool = false,
                isDataless: Bool = false, isPackage: Bool = false, isSymlink: Bool = false,
                isSelected: Bool = false, isCursor: Bool = false, isCutPending: Bool = false) {
        self.index = index; self.name = name; self.kind = kind; self.iconKey = iconKey; self.kindText = kindText
        self.size = size; self.sizeText = sizeText; self.modifiedNs = modifiedNs; self.modifiedText = modifiedText
        self.isHidden = isHidden; self.isDataless = isDataless; self.isPackage = isPackage
        self.isSymlink = isSymlink; self.isSelected = isSelected; self.isCursor = isCursor
        self.isCutPending = isCutPending
    }
}

public enum ListingState: Equatable, Sendable { case loading, complete, error }

/// A tab's current listing without its rows. Fetch rows with `TabAPI.rows`.
public struct ListingInfo: Equatable, Sendable {
    /// Grows with every change; cache rows per version.
    public var version: UInt64
    public var dir: String
    public var state: ListingState
    public var error: String?
    /// Visible rows.
    public var count: UInt32
    /// Every entry, hidden and filtered-out ones included.
    public var totalCount: UInt32
    public init(version: UInt64, dir: String, state: ListingState, error: String?, count: UInt32, totalCount: UInt32) {
        self.version = version; self.dir = dir; self.state = state; self.error = error
        self.count = count; self.totalCount = totalCount
    }
}

/// Why a listing changed.
public enum ChangeKind: Equatable, Sendable { case partial, sorted, patched, resorted, error }

public enum SortBy: Equatable, Hashable, Sendable, CaseIterable { case name, size, modified, created, kind, ext }

/// How a tab shows its folder.
public struct ViewState: Equatable, Sendable {
    public var mode: ViewMode = .list
    public var sort: SortBy = .name
    public var ascending = true
    public var foldersFirst = true
    public var showHidden = false
    public var packagesAsFiles = true
    public init() {}
}

/// The selection.
public struct Selection: Equatable, Sendable {
    public var rows: [UInt32] = []
    public var names: [String] = []
    public var cursor: UInt32?
    public var anchor: UInt32?
    /// The listing version the rows refer to.
    public var version: UInt64 = 0
    public init() {}
}

public enum CursorMove: Equatable, Sendable { case up, down, pageUp, pageDown, home, end }

/// A change to the selection from a click or a key. The core applies the rules.
public enum SelectionChange: Equatable, Sendable {
    /// Plain click.
    case only(UInt32)
    /// ⌘-click.
    case toggle(UInt32)
    /// ⇧-click.
    case extendTo(UInt32)
    case all
    case none
    case invert
    /// A rubber band.
    case rows([UInt32])
    case names([String])
    case move(CursorMove, extend: Bool)
}

/// Where the list was scrolled to (for restore).
public struct ScrollAnchor: Equatable, Sendable {
    /// The top visible row's name.
    public var name: String
    /// Its offset from the top, in points.
    public var offset: Double
    public init(name: String, offset: Double) { self.name = name; self.offset = offset }
}

// MARK: - Workspace

public struct TabState: Equatable, Sendable, Identifiable {
    public var id: TabID
    public var location: String
    public var back: [String]
    public var forward: [String]
    public var view: ViewState
    public var filter: String
    public var selected: [String]
    public var cursor: String?
    public var scroll: ScrollAnchor?
    public init(id: TabID, location: String, back: [String] = [], forward: [String] = [], view: ViewState = ViewState(),
                filter: String = "", selected: [String] = [], cursor: String? = nil, scroll: ScrollAnchor? = nil) {
        self.id = id; self.location = location; self.back = back; self.forward = forward; self.view = view
        self.filter = filter; self.selected = selected; self.cursor = cursor; self.scroll = scroll
    }
    /// The folder's display name.
    public var title: String {
        let n = (location as NSString).lastPathComponent
        return n.isEmpty ? "/" : n
    }
}

public struct PaneState: Equatable, Sendable, Identifiable {
    public var id: PaneID
    public var tabs: [TabState]
    public var activeTab: Int
    public init(id: PaneID, tabs: [TabState], activeTab: Int) { self.id = id; self.tabs = tabs; self.activeTab = activeTab }
    public var activeTabID: TabID? { tabs.indices.contains(activeTab) ? tabs[activeTab].id : tabs.first?.id }
}

public enum Orientation: Equatable, Sendable { case horizontal, vertical }

public struct WindowState: Equatable, Sendable, Identifiable {
    public var id: WindowID
    /// One pane, or two in dual-pane mode.
    public var panes: [PaneState]
    public var activePane: Int
    public var orientation: Orientation
    public var splitRatio: Double
    public init(id: WindowID, panes: [PaneState], activePane: Int, orientation: Orientation = .horizontal,
                splitRatio: Double = 0.5) {
        self.id = id; self.panes = panes; self.activePane = activePane; self.orientation = orientation
        self.splitRatio = splitRatio
    }
    public var activeTabID: TabID? {
        panes.indices.contains(activePane) ? panes[activePane].activeTabID : panes.first?.activeTabID
    }
}

/// Every window, front to back.
public struct WorkspaceState: Equatable, Sendable {
    public var windows: [WindowState]
    public var activeWindow: WindowID?
    public init(windows: [WindowState] = [], activeWindow: WindowID? = nil) {
        self.windows = windows; self.activeWindow = activeWindow
    }
}

public struct WorkspaceSummary: Equatable, Sendable, Identifiable {
    public var id: WorkspaceID
    public var name: String
    public var windows: UInt32
    public var tabs: UInt32
    public var updatedNs: Int64
    public init(id: WorkspaceID, name: String, windows: UInt32, tabs: UInt32, updatedNs: Int64) {
        self.id = id; self.name = name; self.windows = windows; self.tabs = tabs; self.updatedNs = updatedNs
    }
}

// MARK: - Address bar

public enum CrumbKind: Equatable, Sendable { case volumeRoot, home, folder, package }

/// One address-bar segment.
public struct Crumb: Equatable, Sendable, Identifiable {
    public var name: String
    public var path: String
    public var kind: CrumbKind
    public var isCurrent: Bool
    public var id: String { path }
    public init(name: String, path: String, kind: CrumbKind, isCurrent: Bool) {
        self.name = name; self.path = path; self.kind = kind; self.isCurrent = isCurrent
    }
}

/// What address-bar text points at.
public enum Location: Equatable, Sendable {
    case dir(String)
    /// Show `dir` and select `name` (never open it).
    case file(dir: String, name: String)
    /// Offer "Create Folder".
    case missing(path: String, nearestExisting: String)
    case remote(String)
}

public struct ParsedLocation: Equatable, Sendable {
    public var location: Location
    public var cleaned: String
    /// "Cleaned up paste: …" chip text.
    public var chip: String?
    public init(location: Location, cleaned: String, chip: String? = nil) {
        self.location = location; self.cleaned = cleaned; self.chip = chip
    }
}

/// One completion row.
public struct Completion: Equatable, Sendable, Identifiable {
    public var displayName: String
    public var fullPath: String
    /// `~/Pictures/2026`
    public var breadcrumbText: String
    public var isFrecent: Bool
    /// UTF-8 byte ranges of `displayName` to highlight.
    public var matchRanges: [Range<Int>]
    public var score: Double
    /// The whole new input if chosen.
    public var insertText: String
    public var lastVisit: UInt64?
    public var id: String { fullPath }
    public init(displayName: String, fullPath: String, breadcrumbText: String, isFrecent: Bool,
                matchRanges: [Range<Int>], score: Double, insertText: String, lastVisit: UInt64? = nil) {
        self.displayName = displayName; self.fullPath = fullPath; self.breadcrumbText = breadcrumbText
        self.isFrecent = isFrecent; self.matchRanges = matchRanges; self.score = score
        self.insertText = insertText; self.lastVisit = lastVisit
    }
}

/// What Tab does first: replace input[replaceFrom…] (UTF-8 bytes) with `text`.
public struct TabExtension: Equatable, Sendable {
    public var replaceFrom: Int
    public var text: String
    public var newInput: String
    public init(replaceFrom: Int, text: String, newInput: String) {
        self.replaceFrom = replaceFrom; self.text = text; self.newInput = newInput
    }
}

public struct Completions: Equatable, Sendable {
    public var items: [Completion] = []
    public var commonPrefixExtension: TabExtension?
    /// Grey text after the cursor; accept with →.
    public var ghostText = ""
    public var isBareWord = false
    public var parent: String?
    /// UTF-8 byte offset of the last segment.
    public var segmentStart = 0
    /// The parent is a slow (network/cloud) folder and wasn't listed.
    public var skippedSlow = false
    public init() {}
}

public enum PathStyle: Equatable, Sendable, CaseIterable { case posix, escaped, homeRelative, fileURL, quoted }

public struct FrecentDir: Equatable, Sendable {
    public var path: String
    public var score: Double
    public var lastAccessed: UInt64
    public init(path: String, score: Double, lastAccessed: UInt64) {
        self.path = path; self.score = score; self.lastAccessed = lastAccessed
    }
}

// MARK: - Operations

public enum OpKind: Equatable, Sendable { case copy, move, trash, deleteImmediately, newFolder, newFile, rename, duplicate, moveTo }

public enum OpState: Equatable, Sendable {
    case queued, running, paused, waitingForDecision, completed, completedWithErrors, cancelled, failed, interrupted
    public var isFinished: Bool { [.completed, .completedWithErrors, .cancelled, .failed].contains(self) }
}

public enum EntryKind: Equatable, Sendable { case file, dir, symlink, other }

public struct ItemError: Equatable, Sendable {
    public var path: String
    public var dest: String?
    public var message: String
    public var errno: Int32?
    public init(path: String, dest: String? = nil, message: String, errno: Int32? = nil) {
        self.path = path; self.dest = dest; self.message = message; self.errno = errno
    }
}

/// A collision waiting for a decision (the conflict sheet).
public struct Conflict: Equatable, Sendable, Identifiable {
    public var op: OpID
    public var id: ConflictID
    public var source: String
    public var dest: String
    public var sourceKind: EntryKind
    public var destKind: EntryKind
    public var sourceSize: UInt64
    public var destSize: UInt64
    public var sourceMtimeNs: Int64
    public var destMtimeNs: Int64
    /// Both are folders: Merge is offered.
    public var canMerge: Bool
    public init(op: OpID, id: ConflictID, source: String, dest: String, sourceKind: EntryKind, destKind: EntryKind,
                sourceSize: UInt64, destSize: UInt64, sourceMtimeNs: Int64, destMtimeNs: Int64, canMerge: Bool) {
        self.op = op; self.id = id; self.source = source; self.dest = dest; self.sourceKind = sourceKind
        self.destKind = destKind; self.sourceSize = sourceSize; self.destSize = destSize
        self.sourceMtimeNs = sourceMtimeNs; self.destMtimeNs = destMtimeNs; self.canMerge = canMerge
    }
}

public enum ConflictDecision: Equatable, Sendable {
    case keepBoth
    /// The old item goes to the Trash.
    case replace
    case skip
    /// Folders only.
    case merge
}

public struct OpStatus: Equatable, Sendable, Identifiable {
    public var id: OpID
    public var kind: OpKind
    public var title: String
    public var state: OpState
    public var filesDone: UInt64
    public var filesTotal: UInt64
    public var bytesDone: UInt64
    public var bytesTotal: UInt64
    public var current: String?
    public var speedBps: Double
    public var etaSecs: Double?
    public var errors: [ItemError]
    public var conflicts: [Conflict]
    public init(id: OpID, kind: OpKind, title: String, state: OpState, filesDone: UInt64 = 0, filesTotal: UInt64 = 0,
                bytesDone: UInt64 = 0, bytesTotal: UInt64 = 0, current: String? = nil, speedBps: Double = 0,
                etaSecs: Double? = nil, errors: [ItemError] = [], conflicts: [Conflict] = []) {
        self.id = id; self.kind = kind; self.title = title; self.state = state; self.filesDone = filesDone
        self.filesTotal = filesTotal; self.bytesDone = bytesDone; self.bytesTotal = bytesTotal
        self.current = current; self.speedBps = speedBps; self.etaSecs = etaSecs; self.errors = errors
        self.conflicts = conflicts
    }
    /// 0…1, by bytes when known, else by files.
    public var fraction: Double {
        if bytesTotal > 0 { return min(1, Double(bytesDone) / Double(bytesTotal)) }
        if filesTotal > 0 { return min(1, Double(filesDone) / Double(filesTotal)) }
        return state.isFinished ? 1 : 0
    }
}

public struct OpReport: Equatable, Sendable {
    public var id: OpID
    public var kind: OpKind
    public var state: OpState
    public var filesDone: UInt64
    public var filesFailed: UInt64
    public var filesSkipped: UInt64
    public var bytesDone: UInt64
    public var errors: [ItemError]
    public var elapsedMs: UInt64
    public init(id: OpID, kind: OpKind, state: OpState, filesDone: UInt64 = 0, filesFailed: UInt64 = 0,
                filesSkipped: UInt64 = 0, bytesDone: UInt64 = 0, errors: [ItemError] = [], elapsedMs: UInt64 = 0) {
        self.id = id; self.kind = kind; self.state = state; self.filesDone = filesDone; self.filesFailed = filesFailed
        self.filesSkipped = filesSkipped; self.bytesDone = bytesDone; self.errors = errors; self.elapsedMs = elapsedMs
    }
}

/// Edit-menu labels ("Undo Move of 12 Items").
public struct UndoState: Equatable, Sendable {
    public var undo: String?
    public var redo: String?
    public init(undo: String? = nil, redo: String? = nil) { self.undo = undo; self.redo = redo }
}

/// A file operation to queue directly (drag and drop).
public enum OpRequest: Equatable, Sendable {
    case copy(sources: [String], destDir: String)
    case move(sources: [String], destDir: String)
    case trash(paths: [String])
    case newFolder(parent: String, name: String)
    case newFile(parent: String, name: String)
    case rename(path: String, newName: String)
    case duplicate(paths: [String])
}

// MARK: - Sidebar

public struct Volume: Equatable, Sendable, Identifiable {
    public var name: String
    public var mountPath: String
    public var isRemovable: Bool
    public var isNetwork: Bool
    public var isReadOnly: Bool
    public var totalBytes: UInt64
    public var freeBytes: UInt64
    public var id: String { mountPath }
    public init(name: String, mountPath: String, isRemovable: Bool = false, isNetwork: Bool = false,
                isReadOnly: Bool = false, totalBytes: UInt64 = 0, freeBytes: UInt64 = 0) {
        self.name = name; self.mountPath = mountPath; self.isRemovable = isRemovable; self.isNetwork = isNetwork
        self.isReadOnly = isReadOnly; self.totalBytes = totalBytes; self.freeBytes = freeBytes
    }
}

public struct Favourite: Equatable, Sendable, Identifiable {
    public var path: String
    public var name: String
    /// The user gave it its own name.
    public var renamed: Bool
    public var id: String { path }
    public init(path: String, name: String, renamed: Bool = false) { self.path = path; self.name = name; self.renamed = renamed }
}

// MARK: - Context menus (F-022)

public enum SidebarItemKind: Equatable, Sendable { case favourite, volume }

/// What was right-clicked. For list rows get it from `engine.menuTarget(tab:row:)`.
public enum MenuTarget: Equatable, Sendable {
    case file(tab: TabID, path: String)
    case folder(tab: TabID, path: String)
    case package(tab: TabID, path: String)
    case multi(tab: TabID, paths: [String])
    case emptySpace(tab: TabID)
    case crumb(tab: TabID, path: String)
    case sidebarItem(path: String, kind: SidebarItemKind)
    case tab(TabID)
}

public enum MenuKind: Equatable, Sendable, CaseIterable {
    case file, folder, package, multi, emptySpace, crumb, sidebarItem, tab
}

/// A submenu the system provides; the shell fills it (Platform/).
public enum SystemMenu: Equatable, Sendable { case openWith, share, quickActions, services }

/// One menu row. The shell only draws it and performs `invocation` when chosen.
public indirect enum MenuItem: Equatable, Sendable {
    case command(MenuCommand)
    case separator
    case submenu(entry: String, title: String, items: [MenuItem])
    case system(SystemMenu, title: String, paths: [String])
}

public struct MenuCommand: Equatable, Sendable {
    public var entry: String
    public var title: String
    /// The user's current shortcut (key-cap text).
    public var shortcut: String?
    public var enabled: Bool
    public var checked: Bool
    /// A Finder tag colour index (1–7) to draw as a dot.
    public var tagColor: UInt8?
    public var invocation: Invocation
    public init(entry: String, title: String, shortcut: String? = nil, enabled: Bool = true, checked: Bool = false,
                tagColor: UInt8? = nil, invocation: Invocation) {
        self.entry = entry; self.title = title; self.shortcut = shortcut; self.enabled = enabled
        self.checked = checked; self.tagColor = tagColor; self.invocation = invocation
    }
}

/// What the shell knows at the moment of the click.
public struct MenuContext: Equatable, Sendable {
    /// The system pasteboard holds file URLs from another app.
    public var pasteboardHasFiles = false
    public init(pasteboardHasFiles: Bool = false) { self.pasteboardHasFiles = pasteboardHasFiles }
}

/// An entry of a menu, for the Settings editor (`---` is a separator).
public struct MenuEntry: Equatable, Sendable, Identifiable {
    public var entry: String
    public var title: String
    public var id: String { entry }
    public init(entry: String, title: String) { self.entry = entry; self.title = title }
}

// MARK: - Settings

public enum TerminalApp: Equatable, Sendable, CaseIterable {
    case terminal, iTerm, ghostty
    public var bundleID: String {
        switch self {
        case .terminal: "com.apple.Terminal"
        case .iTerm: "com.googlecode.iterm2"
        case .ghostty: "com.mitchellh.ghostty"
        }
    }
    public var title: String {
        switch self {
        case .terminal: "Terminal"
        case .iTerm: "iTerm"
        case .ghostty: "Ghostty"
        }
    }
}

public enum RowDensity: Equatable, Sendable, CaseIterable {
    case compact, regular, comfortable
    /// Row height in points (F-019).
    public var rowHeight: Double {
        switch self {
        case .compact: 20
        case .regular: 22
        case .comfortable: 26
        }
    }
}

public enum Profile: Equatable, Sendable, CaseIterable { case standard, vim }
public enum Corner: Equatable, Sendable, CaseIterable { case topLeft, topRight, bottomLeft, bottomRight }
public enum HintFrequency: Equatable, Sendable, CaseIterable { case always, sometimes, rarely }

/// The settings a Settings window edits. Keys and menus have their own calls.
public struct Settings: Equatable, Sendable {
    public struct General: Equatable, Sendable {
        public var restoreSession = true
        public var startFolder = "~"
        public var dualPaneByDefault = false
        public var confirmTrash = false
        public var typeSelectTimeoutMs: UInt32 = 1000
        public var closedTabsKept: UInt32 = 20
        public var terminalApp: TerminalApp = .terminal
    }
    public struct View: Equatable, Sendable {
        public var mode: ViewMode = .list
        public var sort: SortBy = .name
        public var ascending = true
        public var foldersFirst = true
        public var showHidden = false
        public var packagesAsFiles = true
        public var folderMemory = true
        public var hour24 = true
        public var density: RowDensity = .regular
    }
    public struct PathBar: Equatable, Sendable {
        public var completionLimit: UInt32 = 50
        public var completeHidden = false
        public var frecencyExcluded: [String] = []
        public var maxCrumbs: UInt32 = 6
    }
    public struct Operations: Equatable, Sendable {
        public var verifyCopies = false
        public var fsyncCopies = false
        public var allowDeleteImmediately = false
        public var fileWorkers: UInt32 = 4
        public var opsPerVolume: UInt32 = 1
        public var undoLimit: UInt32 = 100
    }
    public struct Keyboard: Equatable, Sendable {
        public var profile: Profile = .standard
        public var sequenceTimeoutMs: UInt32 = 1000
        public var typingFilters = false
        public var fnKeysAlways = false
    }
    public struct Overlay: Equatable, Sendable {
        public var enabled = true
        public var delayMs: UInt32 = 400
        public var corner: Corner = .bottomRight
        public var listPlainKeys = true
    }
    public struct Hints: Equatable, Sendable {
        public var enabled = true
        public var frequency: HintFrequency = .sometimes
        public var seconds: Float = 4
        public var corner: Corner = .bottomRight
        public var stopWhenLearned = true
    }
    public var general = General()
    public var view = View()
    public var pathBar = PathBar()
    public var operations = Operations()
    public var keyboard = Keyboard()
    public var overlay = Overlay()
    public var hints = Hints()
    public init() {}
}

// MARK: - Events

/// Something the shell should react to. `AppModel` routes these on the main actor.
public enum CoreEvent: Equatable, Sendable {
    case listingChanged(tab: TabID, version: UInt64, kind: ChangeKind)
    case selectionChanged(tab: TabID, count: UInt32, cursor: UInt32?)
    /// Re-read `workspaceState()`.
    case tabsChanged
    case opProgress([OpStatus])
    case opConflict(op: OpID, conflict: Conflict)
    case opFailed(op: OpID, path: String, error: String)
    case opFinished(op: OpID, report: OpReport)
    case undoChanged(UndoState)
    case volumesChanged([Volume])
    case hint(Hint)
    /// Re-read `settings()`.
    case settingsChanged
    /// Re-read `favourites()`.
    case favouritesChanged
}

// MARK: - Protocols

/// One tab: a location, its history, view state, selection and listing.
public protocol TabAPI: AnyObject, Sendable {
    var id: TabID { get }
    /// The folder shown.
    func location() -> String
    func state() -> TabState
    func listingInfo() -> ListingInfo
    /// Rows `start..<start+count`, formatted. Call for visible rows only;
    /// cache by `ListingInfo.version`.
    func rows(start: UInt32, count: UInt32) -> [Row]
    func indexOf(name: String) -> UInt32?
    func pathAt(_ row: UInt32) -> String?
    /// Goes to typed or pasted text; the core parses it.
    func go(_ to: String) throws -> Outcome
    func back() throws -> Outcome
    func forward() throws -> Outcome
    func up() throws -> Outcome
    var canGoBack: Bool { get }
    var canGoForward: Bool { get }
    func refresh()
    func view() -> ViewState
    func setView(_ v: ViewState)
    func filter() -> String
    func setFilter(_ text: String)
    func selection() -> Selection
    func selectedPaths() -> [String]
    func setSelection(_ change: SelectionChange)
    func selectPattern(_ pattern: String, select: Bool)
    /// Type-to-select; returns the row jumped to.
    func typeSelect(_ text: String) -> UInt32?
    func setVisibleRange(start: UInt32, count: UInt32)
    func setScrollAnchor(_ a: ScrollAnchor?)
    func scrollAnchor() -> ScrollAnchor?
    /// The address bar's crumbs.
    func breadcrumbs() -> [Crumb]
    /// Completions for the address-bar text; `cursor` is a UTF-8 byte offset.
    func complete(_ input: String, cursor: UInt32) -> Completions
}

/// The engine: one per process. Owns settings, the workspace, the keymap,
/// context menus, the path engine, the operation queue and the sidebar data.
public protocol EngineAPI: AnyObject, Sendable {
    /// Events arrive on an arbitrary thread; `AppModel` hops to the main actor.
    func setEventHandler(_ handler: @escaping @Sendable (CoreEvent) -> Void)
    /// "Rust" or "Mock", for the About box and debug logs.
    var implementationName: String { get }
    func home() -> String

    // Settings
    func settings() -> Settings
    func updateSettings(_ s: Settings) throws
    /// One setting as JSON by dotted key (`view.show_hidden`).
    func setting(_ key: String) throws -> String
    func setSetting(_ key: String, json: String) throws

    // Workspace
    func restoreSession() -> WorkspaceState
    func workspaceState() -> WorkspaceState
    func tab(_ id: TabID) -> TabAPI?
    func activeTab() -> TabAPI?
    func otherPaneTab(of: TabID) -> TabAPI?
    func newWindow(path: String?) throws -> WindowID
    func closeWindow(_ id: WindowID) throws
    func openTab(path: String?) throws -> TabAPI
    func openTab(inPane pane: PaneID, path: String?, index: UInt32?) throws -> TabAPI
    func closeTab(_ id: TabID) throws
    func closeOtherTabs(keep: TabID) throws
    func reopenClosedTab() throws -> TabAPI?
    func activateTab(_ id: TabID) throws
    func moveTab(_ id: TabID, toPane pane: PaneID, index: UInt32) throws
    func focusPane(window: WindowID, index: UInt32) throws
    func toggleDual(window: WindowID) throws -> Bool
    func setLayout(window: WindowID, orientation: Orientation, splitRatio: Double) throws
    func swapPanes(window: WindowID) throws
    func saveWorkspace(name: String) throws -> WorkspaceID
    func savedWorkspaces() throws -> [WorkspaceSummary]
    func openWorkspace(_ id: WorkspaceID) throws -> WorkspaceState
    func renameWorkspace(_ id: WorkspaceID, name: String) throws
    func deleteWorkspace(_ id: WorkspaceID) throws

    // Commands and keys
    /// What a key press means. `nil`: not ours, let the focused control have it.
    func handleKey(_ chord: KeyChord, context: InputContext) -> Outcome?
    /// Call from a ~100 ms timer while `pendingKeys()` is non-nil.
    func pollKeys(context: InputContext) -> Outcome?
    func pendingKeys() -> String?
    func perform(_ inv: Invocation, via: Via) throws -> Outcome
    func commands() -> [CommandInfo]
    func heldShortcuts(_ mods: Mods, context: InputContext) -> HeldShortcuts?
    func hintFor(command: String, via: Via) -> Hint?
    func vimMode() -> VimMode
    func commandKeys(_ command: String) -> [String]
    func setCommandKey(_ command: String, primary: Bool, keys: String?) throws
    func resetCommandKeys(_ command: String?) throws
    func keymapConflicts() -> [String]
    /// Validates a name typed in place and queues the rename.
    func renameItem(tab: TabID, oldName: String, newName: String) throws -> Outcome
    func createFolderAndGo(tab: TabID, path: String) throws -> Outcome
    /// Files from another app's pasteboard (or cut, pending a move).
    func setClipboard(_ paths: [String], cut: Bool)
    func clipboardPaths() -> [String]

    // Context menus
    /// Applies Finder's right-click rule (`row == nil`: empty space).
    func menuTarget(tab: TabID, row: UInt32?) throws -> MenuTarget
    /// Reads Finder tags (an xattr) of the target items.
    func contextMenu(_ target: MenuTarget, context: MenuContext) throws -> [MenuItem]
    func menuEntries(_ kind: MenuKind) -> [MenuEntry]
    func menuChoices(_ kind: MenuKind) -> [MenuEntry]
    func menuAdd(_ kind: MenuKind, entry: String, after: String?) throws
    func menuRemove(_ kind: MenuKind, entry: String) throws
    func menuMove(_ kind: MenuKind, entry: String, after: String?) throws
    func resetMenu(_ kind: MenuKind) throws
    func resetMenus() throws

    // Path bar
    /// `stat`-like checks only.
    func parseLocation(_ raw: String, cwd: String) -> ParsedLocation
    func breadcrumbs(path: String) -> [Crumb]
    /// Lists the folder (for a crumb's chevron menu).
    func subfolders(path: String) -> [Crumb]
    func complete(_ input: String, cursor: UInt32, cwd: String) -> Completions
    func copyPath(_ path: String, style: PathStyle) -> String
    func frecent(_ query: String, limit: UInt32) -> [FrecentDir]
    /// Imports zoxide's database once (nil: its usual location). Returns how
    /// many folders were added and how many merged into known ones. Reads a file.
    func importZoxide(dbPath: String?) throws -> (added: UInt32, merged: UInt32)

    // Operations
    func enqueue(_ req: OpRequest) throws -> OpID
    func opsList() -> [OpStatus]
    func opStatus(_ id: OpID) -> OpStatus?
    func pauseOp(_ id: OpID) throws
    /// Paused ops, and interrupted ones from an earlier run.
    func resumeOp(_ id: OpID) throws
    /// Interrupted ones too, without resuming them.
    func cancelOp(_ id: OpID) throws
    func reorderOp(_ id: OpID, to index: UInt32) throws
    func resolveConflict(op: OpID, conflict: ConflictID, decision: ConflictDecision, applyToAll: Bool) throws
    /// Reads the journal.
    func interruptedOps() throws -> [OpStatus]
    func clearFinishedOps()
    func undo() throws -> Outcome
    func redo() throws -> Outcome
    func undoState() -> UndoState

    // Sidebar
    func volumes() -> [Volume]
    func favourites() -> [Favourite]
    func addFavourite(_ path: String, index: UInt32?) throws
    func removeFavourite(_ path: String) throws
    func renameFavourite(_ path: String, name: String) throws
    func moveFavourite(_ path: String, to index: UInt32) throws

    // OS hooks
    /// A volume was mounted or unmounted (Platform/VolumeWatcher).
    func volumesChanged()
    /// Save the session and flush before quitting.
    func appWillQuit()
}

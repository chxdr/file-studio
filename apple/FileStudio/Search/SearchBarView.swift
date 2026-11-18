// SearchBarView.swift — the search bar above a pane's list (M3).
//
// Row 1: the search field, the scope (This Folder / This Mac), how the name
// matches, a Filters toggle, the status ("Searching… 120 found") with a
// spinner, and Stop / Close. Row 2 (Filters): kind, size, date, tags,
// content, hidden items, package contents.
//
// Typing searches as you go (debounced); any filter change searches again.
// Esc stops a running search, then closes the bar and shows the folder
// again. ↓ or Return moves to the results. Every control is reachable with
// Tab. The bar only turns controls into a `SearchRequest`; the core runs
// it, words the status and owns the results (F-001).

import AppKit

@MainActor
final class SearchBarView: NSView, NSSearchFieldDelegate, NSTextFieldDelegate {
    static let rowHeight: CGFloat = 34
    static let filterRowHeight: CGFloat = 32

    /// Move focus to the list (↓, Return, and after closing).
    var onFocusList: (() -> Void)?
    /// The bar's height changed (shown, hidden, filters toggled).
    var onHeightChange: ((CGFloat) -> Void)?

    private(set) var tab: TabModel?
    /// Tabs whose bar is open (opened with ⌥⌘F or ⌘F⌘F, not yet closed).
    private var openTabs = Set<TabID>()
    private var showFilters = false
    private var debounce: DispatchWorkItem?
    private var lastError: String?

    let field = SearchBarField()
    private let scope = NSSegmentedControl(labels: ["This Folder", "This Mac"], trackingMode: .selectOne,
                                           target: nil, action: nil)
    private let mode = NSPopUpButton(frame: .zero, pullsDown: false)
    private let filtersButton = NSButton()
    private let spinner = NSProgressIndicator()
    private let status = NSTextField(labelWithString: "")
    private let stopButton = NSButton()

    private let kind = NSPopUpButton(frame: .zero, pullsDown: false)
    private let size = NSPopUpButton(frame: .zero, pullsDown: false)
    private let date = NSPopUpButton(frame: .zero, pullsDown: false)
    private let dateField = NSPopUpButton(frame: .zero, pullsDown: false)
    private let tags = NSTextField()
    private let content = NSTextField()
    private let hiddenBox = NSButton(checkboxWithTitle: "Hidden Items", target: nil, action: nil)
    private let packages = NSButton(checkboxWithTitle: "Package Contents", target: nil, action: nil)
    private let filterRow = NSStackView()

    private static let sizes: [(String, UInt64?, UInt64?)] = [
        ("Any Size", nil, nil), ("Larger than 1 MB", 1_000_000, nil), ("Larger than 100 MB", 100_000_000, nil),
        ("Larger than 1 GB", 1_000_000_000, nil), ("Smaller than 100 KB", nil, 100_000),
        ("Smaller than 1 MB", nil, 1_000_000),
    ]
    private static let dates = ["Any Date", "Today", "Past 7 Days", "Past 30 Days", "Past Year", "Older than a Year"]

    override init(frame: NSRect) {
        super.init(frame: frame)
        build()
        #if DEBUG
        SearchDebugSteps.install()
        #endif
    }
    required init?(coder: NSCoder) { fatalError() }

    // MARK: Building

    private func build() {
        setAccessibilityElement(true)
        setAccessibilityRole(.group)
        setAccessibilityLabel("Search")

        field.placeholderString = "Search"
        field.delegate = self
        field.sendsSearchStringImmediately = true
        field.setAccessibilityLabel("Search for files")
        field.toolTip = "Type to search. ↓ goes to the results; Esc stops, then closes."
        (field.cell as? NSSearchFieldCell)?.cancelButtonCell = nil

        scope.selectedSegment = 0
        scope.target = self; scope.action = #selector(changed)
        scope.controlSize = .small
        scope.setAccessibilityLabel("Where to search")
        scope.toolTip = "Search this folder and below, or the whole Mac"

        mode.addItems(withTitles: SearchNameMode.allCases.map(\.title))
        mode.target = self; mode.action = #selector(changed)
        mode.controlSize = .small
        mode.setAccessibilityLabel("How the name matches")

        filtersButton.setButtonType(.pushOnPushOff)
        filtersButton.bezelStyle = .accessoryBarAction
        filtersButton.image = NSImage(systemSymbolName: "line.3.horizontal.decrease.circle",
                                      accessibilityDescription: "Filters")
        filtersButton.imagePosition = .imageLeading
        filtersButton.title = "Filters"
        filtersButton.controlSize = .small
        filtersButton.target = self; filtersButton.action = #selector(toggleFilters)
        filtersButton.toolTip = "Kind, size, date, tags and content"

        spinner.style = .spinning
        spinner.controlSize = .small
        spinner.isDisplayedWhenStopped = false

        status.font = .systemFont(ofSize: 11)
        status.textColor = .secondaryLabelColor
        status.lineBreakMode = .byTruncatingTail
        status.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        status.setContentHuggingPriority(.defaultLow, for: .horizontal)
        status.setAccessibilityLabel("Search status")

        stopButton.bezelStyle = .accessoryBarAction
        stopButton.isBordered = false
        stopButton.controlSize = .small
        stopButton.target = self; stopButton.action = #selector(stopOrClose)

        kind.addItem(withTitle: "Any Kind")
        kind.addItems(withTitles: SearchKind.allCases.map(\.title))
        size.addItems(withTitles: Self.sizes.map(\.0))
        date.addItems(withTitles: Self.dates)
        dateField.addItems(withTitles: ["Modified", "Created"])
        for (p, label) in [(kind, "Kind"), (size, "Size"), (date, "Date"), (dateField, "Which date")] {
            p.controlSize = .small
            p.target = self; p.action = #selector(changed)
            p.setAccessibilityLabel(label)
        }
        for (f, ph, label) in [(tags, "Tags (comma separated)", "Finder tags"),
                               (content, "Contains text…", "Text in the file (text files only)")] {
            f.placeholderString = ph
            f.controlSize = .small
            f.font = .systemFont(ofSize: 11)
            f.delegate = self
            f.setAccessibilityLabel(label)
            f.bezelStyle = .roundedBezel
        }
        content.toolTip = "Searches text files up to 8 MB; never downloads iCloud files"
        for b in [hiddenBox, packages] {
            b.controlSize = .small
            b.target = self; b.action = #selector(changed)
        }

        let row1 = NSStackView(views: [field, scope, mode, filtersButton, spinner, status, stopButton])
        row1.orientation = .horizontal
        row1.spacing = 8
        row1.edgeInsets = NSEdgeInsets(top: 0, left: 10, bottom: 0, right: 8)
        row1.setHuggingPriority(.defaultLow, for: .horizontal)
        field.widthAnchor.constraint(greaterThanOrEqualToConstant: 160).isActive = true
        let fw = field.widthAnchor.constraint(equalToConstant: 260)
        fw.priority = .defaultLow
        fw.isActive = true

        filterRow.setViews([kind, size, date, dateField, tags, content, hiddenBox, packages], in: .leading)
        filterRow.orientation = .horizontal
        filterRow.spacing = 8
        filterRow.edgeInsets = NSEdgeInsets(top: 0, left: 10, bottom: 0, right: 8)
        for (v, w) in [(kind, 150), (size, 160), (date, 140), (dateField, 96), (tags, 150), (content, 170)]
            as [(NSView, CGFloat)] {
            v.widthAnchor.constraint(equalToConstant: w).isActive = true
        }
        mode.widthAnchor.constraint(lessThanOrEqualToConstant: 230).isActive = true
        mode.setContentHuggingPriority(.defaultHigh, for: .horizontal)
        scope.setContentHuggingPriority(.defaultHigh, for: .horizontal)
        filterRow.isHidden = true

        let sep = NSBox()
        sep.boxType = .separator
        for v in [row1, filterRow, sep] as [NSView] {
            v.translatesAutoresizingMaskIntoConstraints = false
            addSubview(v)
        }
        NSLayoutConstraint.activate([
            row1.topAnchor.constraint(equalTo: topAnchor),
            row1.leadingAnchor.constraint(equalTo: leadingAnchor),
            row1.trailingAnchor.constraint(equalTo: trailingAnchor),
            row1.heightAnchor.constraint(equalToConstant: Self.rowHeight),
            filterRow.topAnchor.constraint(equalTo: row1.bottomAnchor),
            filterRow.leadingAnchor.constraint(equalTo: leadingAnchor),
            filterRow.trailingAnchor.constraint(lessThanOrEqualTo: trailingAnchor),
            filterRow.heightAnchor.constraint(equalToConstant: Self.filterRowHeight),
            sep.leadingAnchor.constraint(equalTo: leadingAnchor),
            sep.trailingAnchor.constraint(equalTo: trailingAnchor),
            sep.bottomAnchor.constraint(equalTo: bottomAnchor),
        ])
        isHidden = true
    }

    override nonisolated var isFlipped: Bool { true }

    var wantedHeight: CGFloat {
        isHidden ? 0 : Self.rowHeight + (showFilters ? Self.filterRowHeight : 0)
    }

    // MARK: Tabs

    private var api: SearchTabAPI? { tab?.api as? SearchTabAPI }

    /// The pane shows `t` (a tab switch).
    func show(_ t: TabModel) {
        if tab?.id != t.id {
            tab = t
            lastError = nil
            if let s = api?.searchStatus() { load(s.request) } else { field.stringValue = "" }
        }
        refresh()
    }

    /// Opens the bar for the current tab and focuses the field. A non-empty
    /// `text` (the filter text, from ⌘F in the filter field) searches at once.
    func open(text: String) {
        guard let t = tab else { return }
        openTabs.insert(t.id)
        if !text.isEmpty {
            field.stringValue = text
            run()
        } else if api?.searchStatus() == nil {
            field.stringValue = ""
        }
        refresh()
        window?.makeFirstResponder(field)
        field.currentEditor()?.selectedRange = NSRange(location: (field.stringValue as NSString).length, length: 0)
    }

    /// Browses a Finder tag (a sidebar tag click): searches the whole Mac for
    /// items with that tag, with the filter row open so the tag is visible.
    func openTag(_ name: String) {
        guard let t = tab else { return }
        openTabs.insert(t.id)
        var r = SearchRequest()
        r.scope = .thisMac
        r.tags = [name]
        load(r)
        filtersButton.state = .on
        showFilters = true
        filterRow.isHidden = false
        run()
        onHeightChange?(wantedHeight)
    }

    /// Reads the status again (after a listing change).
    func refresh() {
        let st = api?.searchStatus()
        let visible = st != nil || (tab.map { openTabs.contains($0.id) } ?? false)
        let wasHidden = isHidden
        isHidden = !visible
        let searching = st?.state == .searching
        if searching { spinner.startAnimation(nil) } else { spinner.stopAnimation(nil) }
        if let e = lastError {
            status.stringValue = e
            status.textColor = .systemRed
        } else {
            status.stringValue = st?.text ?? (field.stringValue.isEmpty ? "Type to search" : "")
            status.textColor = .secondaryLabelColor
        }
        var tip: [String] = []
        if let st {
            if st.deniedCount > 0 {
                tip.append("Couldn’t read:\n" + st.denied.prefix(12).joined(separator: "\n")
                           + (st.deniedCount > 12 ? "\n…" : ""))
            }
            if st.cloudSkipped > 0 { tip.append("\(st.cloudSkipped) iCloud folders not downloaded, not searched") }
            if let e = st.indexError { tip.append("Spotlight: \(e)") }
            if let f = st.firstHitMs { tip.append("First result after \(f) ms; \(st.dirsScanned) folders read") }
        }
        status.toolTip = tip.isEmpty ? nil : tip.joined(separator: "\n\n")
        status.setAccessibilityValue(status.stringValue)
        let stop = searching
        stopButton.image = NSImage(systemSymbolName: stop ? "stop.circle.fill" : "xmark.circle.fill",
                                   accessibilityDescription: stop ? "Stop Search" : "Close Search")
        stopButton.toolTip = stop ? "Stop searching (Esc)" : "Close the search and show the folder (Esc)"
        stopButton.setAccessibilityLabel(stop ? "Stop Search" : "Close Search")
        if wasHidden != isHidden { onHeightChange?(wantedHeight) }
    }

    /// Fills the controls from a request (after a tab switch).
    private func load(_ r: SearchRequest) {
        field.stringValue = r.name
        scope.selectedSegment = r.scope == .thisMac ? 1 : 0
        mode.selectItem(at: SearchNameMode.allCases.firstIndex(of: r.nameMode) ?? 0)
        kind.selectItem(at: r.kinds.first.flatMap { SearchKind.allCases.firstIndex(of: $0) }.map { $0 + 1 } ?? 0)
        size.selectItem(at: Self.sizes.firstIndex { $0.1 == r.minSize && $0.2 == r.maxSize } ?? 0)
        dateField.selectItem(at: r.dateField == .created ? 1 : 0)
        tags.stringValue = r.tags.joined(separator: ", ")
        content.stringValue = r.content ?? ""
        hiddenBox.state = r.includeHidden ? .on : .off
        packages.state = r.includePackages ? .on : .off
    }

    // MARK: Running

    /// The request the controls describe.
    var request: SearchRequest {
        var r = SearchRequest()
        r.name = field.stringValue.trimmingCharacters(in: .whitespaces)
        r.scope = scope.selectedSegment == 1 ? .thisMac : .thisFolder
        r.nameMode = SearchNameMode.allCases[max(mode.indexOfSelectedItem, 0)]
        let k = kind.indexOfSelectedItem
        if k > 0 { r.kinds = [SearchKind.allCases[k - 1]] }
        let s = Self.sizes[max(size.indexOfSelectedItem, 0)]
        r.minSize = s.1; r.maxSize = s.2
        r.dateField = dateField.indexOfSelectedItem == 1 ? .created : .modified
        let now = Date()
        let ns = { (d: Date) in Int64(d.timeIntervalSince1970 * 1_000_000_000) }
        let day: TimeInterval = 86_400
        switch date.indexOfSelectedItem {
        case 1: r.dateFromNs = ns(Calendar.current.startOfDay(for: now))
        case 2: r.dateFromNs = ns(now - 7 * day)
        case 3: r.dateFromNs = ns(now - 30 * day)
        case 4: r.dateFromNs = ns(now - 365 * day)
        case 5: r.dateToNs = ns(now - 365 * day)
        default: break
        }
        r.tags = tags.stringValue.split(separator: ",").map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty }
        let c = content.stringValue.trimmingCharacters(in: .whitespaces)
        r.content = c.isEmpty ? nil : c
        r.includeHidden = hiddenBox.state == .on
        r.includePackages = packages.state == .on
        return r
    }

    /// Runs the search the controls describe (or ends it when they are empty).
    func run() {
        debounce?.cancel()
        guard let api else { return }
        let r = request
        lastError = nil
        if r.isUnfiltered {
            api.endSearch()
        } else {
            do { try api.search(r) } catch { lastError = (error as? LocalizedError)?.errorDescription ?? "\(error)" }
        }
        refresh()
    }

    private func runSoon() {
        debounce?.cancel()
        let w = DispatchWorkItem { [weak self] in MainActor.assumeIsolated { self?.run() } }
        debounce = w
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.3, execute: w)
    }

    @objc private func changed() { run() }

    @objc private func toggleFilters() {
        showFilters = filtersButton.state == .on
        filterRow.isHidden = !showFilters
        onHeightChange?(wantedHeight)
        if showFilters { window?.makeFirstResponder(kind) }
    }

    /// Esc and the stop button: stop a running search, else close the bar.
    @objc func stopOrClose() {
        if let api, api.searchStatus()?.state == .searching {
            api.stopSearch()
            refresh()
            return
        }
        close()
    }

    /// Closes the bar and shows the folder the search started from.
    func close() {
        debounce?.cancel()
        if let t = tab { openTabs.remove(t.id) }
        api?.endSearch()
        lastError = nil
        refresh()
        onFocusList?()
    }

    /// ↓ / Return: focus the results, on the first row when nothing is selected.
    private func goToResults() {
        if let t = tab, t.selectionCount == 0, t.listing.count > 0, t.api.selection().cursor == nil {
            t.api.setSelection(.only(0))
        }
        onFocusList?()
    }

    // MARK: Field delegate

    func controlTextDidChange(_ n: Notification) {
        let o = n.object as AnyObject
        if o === field { runSoon() } else if o === tags || o === content { runSoon() }
    }

    func control(_ control: NSControl, textView: NSTextView, doCommandBy sel: Selector) -> Bool {
        switch sel {
        case #selector(NSResponder.cancelOperation(_:)):
            stopOrClose()
            return true
        case #selector(NSResponder.insertNewline(_:)):
            run()
            goToResults()
            return true
        case #selector(NSResponder.moveDown(_:)) where control === field:
            goToResults()
            return true
        default:
            return false
        }
    }

    // MARK: Debug

    var debugInfo: [String: Any] {
        ["visible": !isHidden, "text": field.stringValue, "status": status.stringValue,
         "scope": scope.selectedSegment == 1 ? "mac" : "folder", "filters": showFilters]
    }

    /// For debug steps: set controls by name.
    func debugSet(_ key: String, _ value: String) {
        switch key {
        case "scope": scope.selectedSegment = value == "mac" ? 1 : 0
        case "mode": mode.selectItem(at: ["contains", "glob", "regex"].firstIndex(of: value) ?? 0)
        case "kind": kind.selectItem(withTitle: value)
        case "size": size.selectItem(withTitle: value)
        case "date": date.selectItem(withTitle: value)
        case "content": content.stringValue = value
        case "tags": tags.stringValue = value
        case "filters":
            filtersButton.state = value == "on" ? .on : .off
            toggleFilters()
            return
        default: field.stringValue = value
        }
        run()
    }
}

/// The search field. It doesn't report a core focus, so typing, Esc and
/// Return stay with the field (global shortcuts still work).
final class SearchBarField: NSSearchField {}

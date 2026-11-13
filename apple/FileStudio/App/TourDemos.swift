// TourDemos.swift — the looping demos on the power-tour cards. Each one is
// built from the app's own views (TourDemoParts.swift: the real list rows,
// pane header, address bar, suggestion list and pane outline; plus the real
// held-⌘ overlay, "Did you know?" hint, toast, queue popover, toolbar ring,
// conflict sheet and Shortcuts key slots), fed a small fixed dataset, so a
// demo looks like the feature because it is drawn by the feature's code.
// What comes from the core is real too: every key bubble is the command's
// current binding, the ⌘ overlay lists the core's held-⌘ shortcuts, and the
// paste chip is the core's own cleanup text. Each demo is a pure function of
// a loop phase `t` (0…1); under Reduce Motion (and in debug snapshots) the
// phase is frozen at the demo's key frame.

import SwiftUI

/// Plays a demo: a 7-second loop, or a frozen frame.
struct TourDemoStage: View {
    let demo: TourCard.Demo
    let keysFor: (String) -> [String]
    let model: AppModel
    let frozenPhase: Double?
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var start = Date()

    private static let period = 7.0

    var body: some View {
        Group {
            if let t = frozenPhase ?? (reduceMotion ? Self.keyFrame(demo) : nil) {
                TourDemo(demo: demo, t: t, keysFor: keysFor, model: model)
            } else {
                TimelineView(.animation(minimumInterval: 1 / 30)) { ctx in
                    let e = ctx.date.timeIntervalSince(start)
                    TourDemo(demo: demo, t: e.truncatingRemainder(dividingBy: Self.period) / Self.period,
                             keysFor: keysFor, model: model)
                }
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(RoundedRectangle(cornerRadius: 12).fill(Color.primary.opacity(0.045)))
        .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(Color.primary.opacity(0.08)))
        .clipShape(RoundedRectangle(cornerRadius: 12))
        .allowsHitTesting(false)
        .onAppear { start = Date() }
    }

    /// The most telling moment of each demo (Reduce Motion shows only this).
    static func keyFrame(_ d: TourCard.Demo) -> Double {
        switch d {
        case .addressBar: 0.5
        case .pasteCleanup: 0.4
        case .dualPane: 0.6
        case .queue: 0.7
        case .cutPaste: 0.3
        case .typeSelect: 0.8
        case .overlay: 0.4
        case .rebind: 0.62
        }
    }
}

// MARK: - Timing helpers (pure)

/// Progress of `t` through [a, b], clamped to 0…1.
func tourSeg(_ t: Double, _ a: Double, _ b: Double) -> Double { min(max((t - a) / (b - a), 0), 1) }
/// Is `t` in [a, b)?
func tourOn(_ t: Double, _ a: Double, _ b: Double) -> Bool { t >= a && t < b }
/// A text caret's blink at phase `t` (on half the time).
func tourBlink(_ t: Double) -> Bool { Int(t * 28) % 2 == 0 }

/// The demo for one card at phase `t`.
private struct TourDemo: View {
    let demo: TourCard.Demo
    let t: Double
    let keysFor: (String) -> [String]
    let model: AppModel

    /// The first current binding of a command (what a key bubble shows).
    func key(_ id: String) -> String { keysFor(id).first ?? "?" }

    var body: some View {
        switch demo {
        case .addressBar: AddressBarDemo(t: t, key: key)
        case .pasteCleanup: PasteDemo(t: t, key: key, chip: PasteDemo.chip(model))
        case .dualPane: DualPaneDemo(t: t, key: key)
        case .queue: QueueDemo(t: t, key: key)
        case .cutPaste: CutPasteDemo(t: t, key: key)
        case .typeSelect: TypeSelectDemo(t: t, key: key)
        case .overlay: OverlayDemo(t: t, key: key, model: model)
        case .rebind: RebindDemo(t: t, key: key, model: model)
        }
    }
}

// MARK: - Shared pieces

/// Keys being pressed, in the demo's bottom-right corner.
private struct KeyBubbles: View {
    /// (text, visible), drawn in place of each other.
    let keys: [(String, Bool)]

    var body: some View {
        ZStack {
            ForEach(keys.indices, id: \.self) { i in
                TourKeyCap(text: keys[i].0, pressed: true)
                    .scaleEffect(keys[i].1 ? 1 : 0.6)
                    .opacity(keys[i].1 ? 1 : 0)
                    .animation(.spring(duration: 0.25, bounce: 0.4), value: keys[i].1)
            }
        }
        .padding(8)
        .background(Capsule().fill(.regularMaterial).opacity(keys.contains { $0.1 } ? 1 : 0))
        .shadow(color: .black.opacity(keys.contains { $0.1 } ? 0.2 : 0), radius: 6, y: 2)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .bottomTrailing)
        .padding(.trailing, 14).padding(.bottom, 30)
    }
}

/// A short caption in the demo's bottom-left corner (what the user is doing).
private struct DemoCaption: View {
    let text: String
    var visible = true

    var body: some View {
        Text(text)
            .font(.system(size: 11, weight: .medium))
            .lineLimit(1)
            .padding(.horizontal, 8).padding(.vertical, 4)
            .background(Capsule().fill(.regularMaterial))
            .overlay(Capsule().strokeBorder(Color.primary.opacity(0.1)))
            .opacity(visible ? 1 : 0)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .bottomLeading)
            .padding(.leading, 12).padding(.bottom, 30)
    }
}

private typealias D = TourDemoData

/// The Documents folder the demos start in (folders first, as the core sorts by name).
@MainActor
private func documents(selected: String? = nil, cursor: String? = nil) -> [Row] {
    [("Design", true, "--"), ("Invoices", true, "--"), ("Reports 2025", true, "--"),
     ("Budget.numbers", false, "220 KB"), ("Notes.txt", false, "4 KB"), ("Report.pdf", false, "1.3 MB")].map {
        D.row($0.0, folder: $0.1, size: $0.2, selected: $0.0 == selected, cursor: $0.0 == cursor)
    }
}

// MARK: - 1. Address bar

private struct AddressBarDemo: View {
    let t: Double
    let key: (String) -> String

    var body: some View {
        let n = Int(tourSeg(t, 0.16, 0.36) * 3.999)
        let tabbed = tourOn(t, 0.46, 0.7)
        let done = t >= 0.7
        let pane: DemoPaneSpec = {
            if done {
                let photos = [("encore.mov", "412 MB"), ("IMG_0412.HEIC", "3.1 MB"), ("IMG_0413.HEIC", "2.9 MB"),
                              ("setlist.jpg", "640 KB")]
                return DemoPaneSpec(crumbs: D.crumbs(["Pictures", "2026", "2026-11-11_gig"]),
                                    bar: .rest(D.crumbs(["Pictures", "2026", "2026-11-11_gig"])), subtitle: D.subtitle(4),
                                    rows: photos.map { D.row($0.0, size: $0.1, modified: "11 Nov 2026 at 23:40") },
                                    status: PaneText.items(4))
            }
            var p = DemoPaneSpec(crumbs: D.crumbs(["Documents"]), bar: .rest(D.crumbs(["Documents"])),
                                 subtitle: D.subtitle(6), rows: documents(), status: PaneText.items(6))
            if t >= 0.04 {
                if tabbed {
                    p.bar = .edit(text: "/Users/you/Pictures/2026/2026-11-11_gig/", caret: tourBlink(t))
                    p.popup = DemoPopup(rows: suggestions(3), highlight: 0)
                } else if n == 0 {
                    p.bar = .edit(text: "/Users/you/Documents/", selected: true)
                } else {
                    p.bar = .edit(text: String("gig".prefix(n)), caret: tourBlink(t))
                    p.popup = DemoPopup(rows: suggestions(n), highlight: nil)
                }
            }
            return p
        }()
        ZStack {
            DemoWindow(panes: [pane]).demoWindowFrame()
            DemoCaption(text: "Typing “gig”", visible: tourOn(t, 0.16, 0.46))
            KeyBubbles(keys: [(key("nav.address_bar"), tourOn(t, 0.02, 0.13)),
                              (key("addressbar.complete_next"), tourOn(t, 0.44, 0.56)),
                              (key("addressbar.commit"), tourOn(t, 0.66, 0.78))])
        }
    }

    /// Suggestions for "g", "gi", "gig": a bare word matches the folders you
    /// visit (frecency), as the core answers it.
    private func suggestions(_ n: Int) -> [PopupRow] {
        let now = UInt64(Date().timeIntervalSince1970)
        let items = [
            Completion(displayName: "2026-11-11_gig", fullPath: "/Users/you/Pictures/2026/2026-11-11_gig",
                       breadcrumbText: "~/Pictures/2026/2026-11-11_gig", isFrecent: true, matchRanges: [11..<(11 + n)],
                       score: 9, insertText: "/Users/you/Pictures/2026/2026-11-11_gig/", lastVisit: now - 2 * 86_400),
            Completion(displayName: "Gigs", fullPath: "/Users/you/Music/Gigs", breadcrumbText: "~/Music/Gigs", isFrecent: true,
                       matchRanges: [0..<n], score: 5, insertText: "/Users/you/Music/Gigs/", lastVisit: now - 5 * 3_600),
        ]
        return items.map(PathBarView.completionRow)
    }
}

// MARK: - 2. Paste cleanup and Create Folder

private struct PasteDemo: View {
    let t: Double
    let key: (String) -> String
    let chip: String?

    static let raw = "\"file:///Users/you/My%20Projects/new%20idea/ \""
    static let cleaned = "/Users/you/My Projects/new idea"
    private static var chipCache: String??

    /// The core's own cleanup text for the pasted string (asked once).
    static func chip(_ model: AppModel) -> String? {
        if let c = chipCache { return c }
        let c = model.engine.parseLocation(raw, cwd: TourDemoData.home + "/Documents").chip
        chipCache = .some(c)
        return c
    }

    var body: some View {
        let pasted = t >= 0.16
        let offered = tourOn(t, 0.56, 0.9)
        let created = t >= 0.9
        var pane = DemoPaneSpec(crumbs: D.crumbs(["Documents"]), bar: .rest(D.crumbs(["Documents"])),
                                subtitle: D.subtitle(6), rows: documents(), status: PaneText.items(6))
        if created {
            pane = DemoPaneSpec(crumbs: D.crumbs(["My Projects", "new idea"]), bar: .rest(D.crumbs(["My Projects", "new idea"])),
                                subtitle: D.subtitle(0), rows: [], status: PaneText.items(0))
        } else if offered {
            let o = PathBarView.offerRows(path: Self.cleaned, nearest: D.home + "/My Projects", home: D.home)
            pane.bar = .edit(text: Self.cleaned, caret: tourBlink(t), error: true)
            pane.popup = DemoPopup(rows: o.rows, highlight: o.rows.firstIndex { $0.payload == o.defaultPayload })
        } else if pasted {
            pane.bar = .edit(text: Self.cleaned, caret: tourBlink(t), chip: chip)
        } else if t >= 0.04 {
            pane.bar = .edit(text: "/Users/you/Documents/", selected: true)
        }
        return ZStack {
            DemoWindow(panes: [pane]).demoWindowFrame()
            DemoCaption(text: "On the clipboard:  \(Self.raw)", visible: t < 0.5)
            KeyBubbles(keys: [(key("nav.address_bar"), tourOn(t, 0.02, 0.1)),
                              (key("edit.paste"), tourOn(t, 0.12, 0.24)),
                              (key("addressbar.commit"), tourOn(t, 0.52, 0.62) || tourOn(t, 0.84, 0.94))])
        }
    }
}

// MARK: - 3. Dual pane

private struct DualPaneDemo: View {
    let t: Double
    let key: (String) -> String

    var body: some View {
        let dual = t >= 0.14
        let copied = t >= 0.42
        let right = t >= 0.72
        let cols: [(ColumnID, CGFloat)] = [(.name, 0), (.size, 56), (.modified, 122)]
        let downloads = [("fonts.zip", "6.4 MB"), ("Invoice.pdf", "88 KB"), ("scan.jpg", "1.2 MB")]
        let left = DemoPaneSpec(crumbs: D.crumbs(["Downloads"]), bar: .rest(D.crumbs(["Downloads"])), subtitle: D.subtitle(3),
                                rows: downloads.map { D.row($0.0, size: $0.1, selected: $0.0 == "Invoice.pdf") },
                                columns: dual ? cols : [(.name, 0), (.size, 70), (.modified, 140), (.kind, 120)],
                                marked: dual && !right, focused: !right,
                                status: "1 of 3 items selected, 88 KB")
        var accounts = [("Budget.numbers", "220 KB"), ("Notes.txt", "4 KB")]
        if copied { accounts.insert(("Invoice.pdf", "88 KB"), at: 1) }
        let rightPane = DemoPaneSpec(crumbs: D.crumbs(["Accounts 2026"]), bar: .rest(D.crumbs(["Accounts 2026"])),
                                     subtitle: D.subtitle(accounts.count),
                                     rows: accounts.map { D.row($0.0, size: $0.1, selected: copied && $0.0 == "Invoice.pdf") },
                                     columns: cols, marked: right, focused: right,
                                     status: copied ? "1 of 3 items selected, 88 KB" : PaneText.items(UInt32(accounts.count)))
        return ZStack {
            DemoWindow(panes: dual ? [left, rightPane] : [left]).demoWindowFrame()
            KeyBubbles(keys: [(key("pane.toggle_dual"), tourOn(t, 0.04, 0.16)),
                              (key("file.copy_to_other_pane"), tourOn(t, 0.3, 0.44)),
                              (key("pane.switch"), tourOn(t, 0.66, 0.78))])
        }
    }
}

// MARK: - 4. Operation queue

private struct QueueDemo: View {
    let t: Double
    let key: (String) -> String

    var body: some View {
        let conflict = tourOn(t, 0.5, 0.82)
        let done = t >= 0.84
        let swapped = t >= 0.3
        let mb: UInt64 = 1_000_000
        let copyDone = UInt64((0.15 + 0.5 * min(t, 0.5) / 0.5) * 1800) * mb
        let a = OpStatus(id: 1, kind: .copy, title: "Copying 1,204 items to “Backup”",
                         state: done ? .completed : (conflict ? .waitingForDecision : .running),
                         filesDone: done ? 1204 : UInt64(Double(copyDone) / Double(1800 * mb) * 1204), filesTotal: 1204,
                         bytesDone: done ? 1800 * mb : copyDone, bytesTotal: 1800 * mb, current: "/Users/you/Documents/Report.pdf",
                         speedBps: 92 * Double(mb), etaSecs: 14)
        let b = OpStatus(id: 2, kind: .move, title: "Moving “Photos” to “Archive”", state: .paused, filesDone: 310,
                         filesTotal: 742, bytesDone: 2_100 * mb, bytesTotal: 5_000 * mb, current: "/Users/you/Pictures/IMG_0310.HEIC")
        let c = OpStatus(id: 3, kind: .copy, title: "Copying “Projects” to “NAS”", state: .queued, filesTotal: 88,
                         bytesTotal: 640 * mb)
        let active = done ? (swapped ? [c, b] : [b, c]) : (swapped ? [a, c, b] : [a, b, c])
        let ring = OpsRing(fraction: done ? 0.4 : Double(copyDone) / Double(1800 * mb) * 0.6, running: true, allPaused: false,
                           conflicts: conflict ? 1 : 0, failures: 0, interrupted: 0, hasFinished: done)
        let conflictInfo = Conflict(op: 1, id: 1, source: "/Users/you/Documents/Report.pdf", dest: "/Volumes/Backup/Report.pdf",
                                    sourceKind: .file, destKind: .file, sourceSize: 1_300_000, destSize: 1_100_000,
                                    sourceMtimeNs: Int64((Date().timeIntervalSince1970 - 3_600) * 1e9),
                                    destMtimeNs: Int64((Date().timeIntervalSince1970 - 90_000) * 1e9), canMerge: false)
        let backup = [D.row("Projects", folder: true), D.row("Report.pdf", size: "1.1 MB"),
                      D.row("Tax 2025.pdf", size: "640 KB")]
        let window = DemoPaneSpec(crumbs: [Crumb(name: "Backup", path: "/Volumes/Backup", kind: .volumeRoot, isCurrent: true)],
                                  bar: .rest([Crumb(name: "Backup", path: "/Volumes/Backup", kind: .volumeRoot, isCurrent: true)]),
                                  subtitle: D.subtitle(3, free: "1.2 TB"), rows: backup, status: PaneText.items(3),
                                  message: "Copying 1,204 items to “Backup”: waiting for you", free: "1.2 TB")
        return ZStack(alignment: .top) {
            if conflict {
                // The sheet comes down from the top of the window, as on screen.
                DemoWindow(panes: [window]).demoWindowFrame()
                KeyInert {
                    ConflictCard(conflict: conflictInfo, answered: 0, total: 1, remaining: 1,
                                 applyToAll: .constant(false), decide: { _ in }, drawingOnly: true)
                        .background(Color(nsColor: .windowBackgroundColor))
                }
                .fixedSize()
                .scaleEffect(0.62, anchor: .topLeading)
                .frame(width: 600 * 0.62, height: 238, alignment: .topLeading)
                .clipShape(UnevenRoundedRectangle(bottomLeadingRadius: 10, bottomTrailingRadius: 10))
                .shadow(color: .black.opacity(0.35), radius: 12, y: 5)
                .offset(y: -238 * (1 - tourSeg(t, 0.5, 0.55)))
                .padding(.top, 1)
            } else {
                HStack(alignment: .top, spacing: 8) {
                    Spacer(minLength: 0)
                    // The queue popover, at 3/4 size so all three operations fit.
                    OpsQueueContent(active: active, finished: done ? [a] : [], interrupted: [], ops: nil)
                        .background(RoundedRectangle(cornerRadius: 12).fill(.regularMaterial))
                        .clipShape(RoundedRectangle(cornerRadius: 12))
                        .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(Color.primary.opacity(0.12)))
                        .scaleEffect(0.74, anchor: .topLeading)
                        .frame(width: 440 * 0.74, height: 270, alignment: .topLeading)
                        .shadow(color: .black.opacity(0.2), radius: 8, y: 3)
                    // The toolbar ring the popover hangs from.
                    VStack(spacing: 3) {
                        ring
                        Text("Operations").font(.system(size: 10)).foregroundStyle(.secondary)
                    }
                    .padding(6)
                    .background(RoundedRectangle(cornerRadius: 8).fill(Color.primary.opacity(0.06)))
                    Spacer(minLength: 0)
                }
            }
            KeyBubbles(keys: [(key("edit.undo"), tourOn(t, 0.9, 0.99))])
        }
    }
}

// MARK: - 5. Cut and paste to move

private struct CutPasteDemo: View {
    let t: Double
    let key: (String) -> String

    private static let marked = Toast(text: "1 item marked to move; paste to move it", style: .info, seconds: 3)
    private static let moved = Toast(text: "Moved “plan.key” to “Projects”", style: .success, seconds: 6,
                                     actionTitle: "Undo", action: {})

    var body: some View {
        let cut = t >= 0.18
        let right = t >= 0.44
        let moved = t >= 0.62
        let cols: [(ColumnID, CGFloat)] = [(.name, 0), (.size, 56), (.modified, 122)]
        var desk = [D.row("plan.key", size: "8.4 MB", selected: !moved, cut: cut && !moved),
                    D.row("screenshot.png", size: "640 KB")]
        if moved { desk.removeFirst() }
        var proj = [D.row("Archive", folder: true)]
        if moved { proj.append(D.row("plan.key", size: "8.4 MB", selected: true)) }
        let left = DemoPaneSpec(crumbs: D.crumbs(["Desktop"]), bar: .rest(D.crumbs(["Desktop"])), subtitle: D.subtitle(desk.count),
                                rows: desk, columns: cols, marked: !right, focused: !right,
                                status: moved ? PaneText.items(1) : "1 of 2 items selected, 8.4 MB",
                                message: tourOn(t, 0.18, 0.44) ? Self.marked.text : nil)
        let rightPane = DemoPaneSpec(crumbs: D.crumbs(["Projects"]), bar: .rest(D.crumbs(["Projects"])),
                                     subtitle: D.subtitle(proj.count), rows: proj, columns: cols, marked: right, focused: right,
                                     status: moved ? "1 of 2 items selected, 8.4 MB" : PaneText.items(1))
        let toast: Toast? = moved ? Self.moved : (tourOn(t, 0.18, 0.44) ? Self.marked : nil)
        return ZStack {
            DemoWindow(panes: [left, rightPane]).demoWindowFrame()
            if let toast {
                ToastCard(toast: toast)
                    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .bottom)
                    .padding(.bottom, 32)
                    .id(toast.id)
            }
            KeyBubbles(keys: [(key("edit.cut"), tourOn(t, 0.1, 0.22)), (key("pane.switch"), tourOn(t, 0.38, 0.48)),
                              (key("edit.paste"), tourOn(t, 0.54, 0.66))])
        }
    }
}

// MARK: - 6. Type to select vs filter

private struct TypeSelectDemo: View {
    let t: Double
    let key: (String) -> String

    var body: some View {
        let typed = String("no".prefix(Int(tourSeg(t, 0.06, 0.24) * 2.999)))
        let filterOn = tourOn(t, 0.5, 0.88)
        let filter = String("rep".prefix(Int(tourSeg(t, 0.56, 0.74) * 3.999)))
        // Type-to-select is a prefix match in list order (folders first): "n" lands on Notes.txt.
        let jumped = typed.isEmpty ? (t >= 0.24 ? "Notes.txt" : nil)
            : documents().first { $0.name.lowercased().hasPrefix(typed) }?.name
        let all = documents(selected: jumped, cursor: jumped)
        let shown = filterOn && !filter.isEmpty ? all.filter { $0.name.lowercased().contains(filter) } : all
        let selectedShown = shown.contains { $0.isSelected }
        // While typing in the filter field the list keeps its selection, unemphasized.
        let pane = DemoPaneSpec(crumbs: D.crumbs(["Documents"]), bar: .rest(D.crumbs(["Documents"])),
                                subtitle: D.subtitle(shown.count), rows: shown, focused: !filterOn,
                                status: !selectedShown ? PaneText.items(UInt32(shown.count))
                                    : "1 of \(PaneText.items(UInt32(shown.count))) selected, 4 KB")
        return ZStack {
            VStack(spacing: 0) {
                // The toolbar's Filter field (WindowToolbar), at its place on the right.
                HStack {
                    Spacer()
                    DemoSearchField(text: filterOn ? filter : "", focused: filterOn)
                        .frame(width: 200, height: 22)
                }
                .padding(.horizontal, 10)
                .frame(height: 38)
                .background(Color(nsColor: .windowBackgroundColor))
                .overlay(alignment: .bottom) { Divider() }
                DemoWindow(panes: [pane])
            }
            .demoWindowFrame()
            DemoCaption(text: filterOn ? "Filter “\(filter)”: only what matches" : "Typing “\(typed.isEmpty ? "no" : typed)” selects the first match",
                        visible: (t >= 0.06 && t < 0.42) || filterOn)
            KeyBubbles(keys: [(key("filter.focus"), tourOn(t, 0.44, 0.56)), (key("filter.clear"), tourOn(t, 0.84, 0.96))])
        }
    }
}

/// The toolbar's search field (an NSSearchField, as WindowToolbar makes it).
private struct DemoSearchField: NSViewRepresentable {
    let text: String
    let focused: Bool

    func makeNSView(context: Context) -> NSSearchField {
        let f = NSSearchField()
        f.placeholderString = "Filter"
        f.isEditable = false
        f.focusRingType = .exterior
        return f
    }
    func updateNSView(_ f: NSSearchField, context: Context) {
        if f.stringValue != text { f.stringValue = text }
    }
}

// MARK: - 7. Hold ⌘ for the overlay, and hints

private struct OverlayDemo: View {
    let t: Double
    let key: (String) -> String
    let model: AppModel
    @State private var scroll = HeldScrollState()

    var body: some View {
        let held = tourOn(t, 0.1, 0.72)
        let hint = t >= 0.78
        let rows = Self.heldRows(model)
        let corner = model.settings.overlay.corner
        let pane = DemoPaneSpec(crumbs: D.crumbs(["Documents"]), bar: .rest(D.crumbs(["Documents"])), subtitle: D.subtitle(6),
                                rows: documents(selected: "Report.pdf", cursor: "Report.pdf"),
                                status: "1 of 6 items selected, 1.3 MB")
        return ZStack {
            DemoWindow(panes: [pane]).demoWindowFrame()
            if held, let rows {
                GeometryReader { g in
                    HeldShortcutsView(held: rows, corner: corner, appearance: AppearanceModel.of(model),
                                      available: CGSize(width: g.size.width - 24, height: g.size.height - 44),
                                      scroll: scroll, animated: false)
                        .padding(.top, 12).padding(.bottom, 32).padding(.horizontal, 12)
                        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: corner.alignment)
                }
                .transition(.opacity)
            }
            if hint {
                KeyInert {
                    HintToast(model: model, hint: Hint(command: "file.duplicate", keys: key("file.duplicate"), label: "Duplicate"),
                              onDismiss: {})
                }
                .fixedSize()
                .padding(.top, 12).padding(.bottom, 32).padding(.horizontal, 12)
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: model.settings.hints.corner.alignment)
            }
            DemoCaption(text: "Duplicated a file from its right-click menu", visible: hint)
        }
        // The wheel scrolls the list while ⌘ stays down (frozen frames show the top).
        .onChange(of: t) { scroll.offset = 160 * tourSeg(t, 0.46, 0.62) }
    }

    /// The core's own held-⌘ list (as the overlay shows it in the list).
    static func heldRows(_ model: AppModel) -> HeldShortcuts? {
        var m = Mods()
        m.cmd = true
        return model.engine.heldShortcuts(m, context: InputContext(focus: .list, tab: nil))
    }
}

// MARK: - 8. Everything is rebindable

private struct RebindDemo: View {
    let t: Double
    let key: (String) -> String
    let model: AppModel

    var body: some View {
        let recording = tourOn(t, 0.25, 0.5)
        let rebound = t >= 0.5
        let vim = t >= 0.78
        let keyboardTab = t >= 0.7
        VStack(spacing: 10) {
            Picker("", selection: .constant(keyboardTab ? ShortcutsSection.behaviour : .commands)) {
                ForEach(ShortcutsSection.allCases, id: \.self) { s in Text(s == .customised && rebound ? "Customised (1)" : s.rawValue).tag(s) }
            }
            .pickerStyle(.segmented).labelsHidden()
            .padding(.horizontal, 16).padding(.top, 12)
            if keyboardTab {
                Form {
                    Section("Keyboard") {
                        Picker("Shortcut profile", selection: .constant(vim ? Profile.vim : .standard)) {
                            Text("Standard").tag(Profile.standard)
                            Text("Vim").tag(Profile.vim)
                        }
                        Picker("Typing in a list", selection: .constant(false)) {
                            Text("Selects the matching item").tag(false)
                            Text("Filters the list").tag(true)
                        }
                    }
                }
                .formStyle(.grouped)
                .scrollDisabled(true)
            } else {
                VStack(spacing: 0) {
                    // SettingsShortcutsPane.table's search field and header.
                    HStack {
                        Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
                        Text("Search commands, categories or keys").foregroundStyle(.tertiary)
                        Spacer()
                    }
                    .padding(8).background(RoundedRectangle(cornerRadius: 8).fill(Color.primary.opacity(0.06)))
                    .padding(.horizontal, 16)
                    HStack {
                        Text("Command").frame(maxWidth: .infinity, alignment: .leading)
                        Text("Primary").frame(width: 130)
                        Text("Secondary").frame(width: 130)
                        Color.clear.frame(width: 28, height: 1)
                    }
                    .font(.caption.weight(.semibold)).foregroundStyle(.secondary)
                    .padding(.horizontal, 24).padding(.top, 10).padding(.bottom, 4)
                    Divider()
                    row("file.copy_to_other_pane", primary: rebound ? "⌃⌥C" : key("file.copy_to_other_pane"),
                        recording: recording, customised: rebound)
                    Divider().padding(.leading, 24)
                    row("file.move_to_other_pane", primary: key("file.move_to_other_pane"), recording: false, customised: false)
                    Divider().padding(.leading, 24)
                    row("pane.toggle_dual", primary: key("pane.toggle_dual"), recording: false, customised: false)
                    Spacer(minLength: 0)
                }
            }
        }
        .background(Color(nsColor: .windowBackgroundColor))
        .demoWindowFrame()
        .overlay {
            KeyBubbles(keys: [("⌃⌥C", tourOn(t, 0.42, 0.52))])
        }
    }

    /// A row of Settings › Shortcuts (SettingsShortcutsPane.row) with its real key
    /// slots; the title and category are the core's.
    private func row(_ id: String, primary: String, recording: Bool, customised: Bool) -> some View {
        let c = model.engine.commands().first { $0.id == id }
        return HStack {
            VStack(alignment: .leading, spacing: 1) {
                Text(c?.title ?? id).lineLimit(1)
                Text(c?.category ?? "").font(.caption).foregroundStyle(.secondary)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            KeySlot(text: primary, recording: recording, customised: customised, action: {}, clear: {}).frame(width: 130)
            KeySlot(text: nil, recording: false, customised: false, action: {}, clear: {}).frame(width: 130)
            Image(systemName: "arrow.uturn.backward").foregroundStyle(.secondary)
                .opacity(customised ? 1 : 0).frame(width: 28)
        }
        .padding(.horizontal, 24).padding(.vertical, 5)
    }
}

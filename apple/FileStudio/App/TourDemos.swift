// TourDemos.swift — the small looping demos on the power-tour cards. They
// are drawn (SwiftUI shapes in the app's look: ForkLift-like rows, accent
// outlines), not live app views: a live pane would need a real folder, tab
// and queue. What they show is real where it can be: every key bubble is the
// command's current binding, and the ⌘ overlay demo lists the core's own
// held-⌘ shortcuts. Each demo is a pure function of a loop phase `t` (0…1);
// under Reduce Motion (and in debug snapshots) the phase is frozen.

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
        .padding(14)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(RoundedRectangle(cornerRadius: 12).fill(Color.primary.opacity(0.045)))
        .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(Color.primary.opacity(0.08)))
        .onAppear { start = Date() }
    }

    /// The most telling moment of each demo (Reduce Motion shows only this).
    static func keyFrame(_ d: TourCard.Demo) -> Double {
        switch d {
        case .addressBar: 0.55
        case .pasteCleanup: 0.8
        case .dualPane: 0.6
        case .queue: 0.7
        case .cutPaste: 0.45
        case .typeSelect: 0.8
        case .overlay: 0.75
        case .rebind: 0.62
        }
    }
}

// MARK: - Timing helpers (pure)

/// Progress of `t` through [a, b], clamped to 0…1.
func tourSeg(_ t: Double, _ a: Double, _ b: Double) -> Double { min(max((t - a) / (b - a), 0), 1) }
/// Is `t` in [a, b)?
func tourOn(_ t: Double, _ a: Double, _ b: Double) -> Bool { t >= a && t < b }

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
        case .pasteCleanup: PasteDemo(t: t, key: key)
        case .dualPane: DualPaneDemo(t: t, key: key)
        case .queue: QueueDemo(t: t, key: key)
        case .cutPaste: CutPasteDemo(t: t, key: key)
        case .typeSelect: TypeSelectDemo(t: t, key: key)
        case .overlay: OverlayDemo(t: t, key: key, rows: heldRows)
        case .rebind: RebindDemo(t: t, key: key)
        }
    }

    /// The core's own ⌘ overlay rows (the first eight).
    private var heldRows: [(String, String)] {
        var m = Mods()
        m.cmd = true
        let rows = model.engine.heldShortcuts(m, context: InputContext(focus: .list, tab: nil))?.rows ?? []
        return rows.prefix(8).map { ("⌘" + $0.keys, $0.label) }
    }
}

// MARK: - Shared pieces

/// A key being pressed, shown in the demo's top-right corner.
private struct KeyBubble: View {
    let text: String
    let visible: Bool

    var body: some View {
        TourKeyCap(text: text, pressed: true)
            .scaleEffect(visible ? 1 : 0.6)
            .opacity(visible ? 1 : 0)
            .animation(.spring(duration: 0.25, bounce: 0.4), value: visible)
    }
}

/// A list row in the app's look: icon, name, optional detail.
private struct DemoRow: View {
    let icon: String
    let name: String
    var detail: String = ""
    var selected = false
    var dimmed = false
    var focused = true

    var body: some View {
        HStack(spacing: 7) {
            Image(systemName: icon).font(.system(size: 12)).foregroundStyle(icon.hasPrefix("folder") ? Color.accentColor : .secondary)
                .frame(width: 16)
            Text(name).font(.system(size: 12)).lineLimit(1)
            Spacer(minLength: 4)
            if !detail.isEmpty { Text(detail).font(.system(size: 11).monospacedDigit()).foregroundStyle(.secondary) }
        }
        .padding(.horizontal, 7)
        .frame(height: 21)
        .foregroundStyle(selected && focused ? Color.white : Color.primary)
        .background(RoundedRectangle(cornerRadius: 5).fill(selected ? (focused ? Color.accentColor : Color.secondary.opacity(0.25)) : .clear))
        .opacity(dimmed ? 0.45 : 1)
    }
}

/// A pane: a header with the folder name and rows, outlined when active.
private struct DemoPane<Content: View>: View {
    let title: String
    var active = true
    @ViewBuilder let content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(spacing: 5) {
                Image(systemName: "folder.fill").font(.system(size: 11)).foregroundStyle(.tint)
                Text(title).font(.system(size: 12, weight: .semibold))
            }
            .padding(.horizontal, 7).padding(.vertical, 5)
            content
            Spacer(minLength: 0)
        }
        .padding(4)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(RoundedRectangle(cornerRadius: 8).fill(Color(nsColor: .textBackgroundColor)))
        .overlay(alignment: .top) {
            if active { Rectangle().fill(Color.accentColor).frame(height: 3).clipShape(RoundedRectangle(cornerRadius: 2)) }
        }
        .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(active ? Color.accentColor : Color.primary.opacity(0.12),
                                                                lineWidth: active ? 2 : 1))
    }
}

/// The address field: a capsule with a location glyph.
private struct DemoField<Content: View>: View {
    var focused = true
    @ViewBuilder let content: Content

    var body: some View {
        HStack(spacing: 6) {
            Image(systemName: "folder").font(.system(size: 12)).foregroundStyle(.secondary)
            content
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 10)
        .frame(height: 28)
        .background(Capsule().fill(Color(nsColor: .textBackgroundColor)))
        .overlay(Capsule().strokeBorder(focused ? Color.accentColor : Color.primary.opacity(0.15), lineWidth: focused ? 2 : 1))
    }
}

/// A blinking text caret.
private struct Caret: View {
    let t: Double
    var body: some View {
        Rectangle().fill(Color.accentColor).frame(width: 1.5, height: 15)
            .opacity(Int(t * 28) % 2 == 0 ? 1 : 0.15)
    }
}

// MARK: - 1. Address bar

private struct AddressBarDemo: View {
    let t: Double
    let key: (String) -> String

    var body: some View {
        let typed = String("gig".prefix(Int(tourSeg(t, 0.12, 0.38) * 3.999)))
        let committed = t >= 0.74
        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .top) {
                DemoField(focused: !committed) {
                    if committed {
                        crumbs
                    } else if t < 0.08 {
                        Text("~/Documents").font(.system(size: 13)).foregroundStyle(.secondary)
                    } else {
                        HStack(spacing: 0) {
                            Text(typed).font(.system(size: 13))
                            Caret(t: t)
                            if tourOn(t, 0.45, 0.74) {
                                Text("  →  ~/Pictures/2026/2026-11-11_gig").font(.system(size: 13)).foregroundStyle(.secondary)
                            }
                        }
                    }
                }
                Spacer(minLength: 12)
                ZStack {
                    KeyBubble(text: key("nav.address_bar"), visible: tourOn(t, 0.02, 0.12))
                    KeyBubble(text: key("addressbar.complete_next"), visible: tourOn(t, 0.45, 0.6))
                    KeyBubble(text: key("addressbar.commit"), visible: tourOn(t, 0.68, 0.82))
                }
                .frame(width: 70)
            }
            if tourOn(t, 0.3, 0.74) {
                VStack(alignment: .leading, spacing: 1) {
                    suggestion("2026-11-11_gig", "Pictures › 2026", selected: true)
                    suggestion("gig-posters", "Documents › Design", selected: false)
                    suggestion("Gigabit logs", "Library › Logs", selected: false)
                }
                .padding(5)
                .background(RoundedRectangle(cornerRadius: 8).fill(Color(nsColor: .windowBackgroundColor)).shadow(radius: 4, y: 2))
                .frame(maxWidth: 380)
                .transition(.opacity)
            }
            if committed {
                DemoPane(title: "2026-11-11_gig") {
                    DemoRow(icon: "photo", name: "IMG_0412.HEIC", detail: "3.1 MB", selected: true)
                    DemoRow(icon: "photo", name: "IMG_0413.HEIC", detail: "2.9 MB")
                    DemoRow(icon: "film", name: "encore.mov", detail: "412 MB")
                }
                .frame(maxWidth: 380, maxHeight: 110)
            }
            Spacer(minLength: 0)
        }
    }

    private var crumbs: some View {
        HStack(spacing: 3) {
            ForEach(["Pictures", "2026", "2026-11-11_gig"], id: \.self) { c in
                Text(c).font(.system(size: 13, weight: c.hasSuffix("gig") ? .semibold : .regular))
                if !c.hasSuffix("gig") { Image(systemName: "chevron.right").font(.system(size: 8)).foregroundStyle(.secondary) }
            }
        }
    }

    private func suggestion(_ name: String, _ where_: String, selected: Bool) -> some View {
        HStack(spacing: 7) {
            Image(systemName: "folder.fill").font(.system(size: 12)).foregroundStyle(selected ? .white : .accentColor)
            Text(name).font(.system(size: 12, weight: .medium))
            Text(where_).font(.system(size: 11)).foregroundStyle(selected ? .white.opacity(0.8) : .secondary)
            Spacer()
        }
        .foregroundStyle(selected ? .white : .primary)
        .padding(.horizontal, 7).frame(height: 22)
        .background(RoundedRectangle(cornerRadius: 5).fill(selected ? Color.accentColor : .clear))
    }
}

// MARK: - 2. Paste cleanup and Create Folder

private struct PasteDemo: View {
    let t: Double
    let key: (String) -> String

    var body: some View {
        let cleaned = t >= 0.3
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .top) {
                DemoField {
                    if cleaned {
                        Text("~/My Projects/new idea").font(.system(size: 13))
                        Label("cleaned up", systemImage: "sparkles")
                            .font(.system(size: 10, weight: .semibold))
                            .padding(.horizontal, 6).padding(.vertical, 2)
                            .background(Capsule().fill(Color.accentColor.opacity(0.18)))
                            .foregroundStyle(.tint)
                            .scaleEffect(0.85 + 0.15 * tourSeg(t, 0.3, 0.38))
                    } else if t >= 0.1 {
                        Text("\"file:///Users/me/My%20Projects/new%20idea/ \"")
                            .font(.system(size: 12, design: .monospaced)).foregroundStyle(.secondary).lineLimit(1)
                    }
                }
                .frame(maxWidth: 440)
                Spacer(minLength: 12)
                ZStack {
                    KeyBubble(text: key("edit.paste"), visible: tourOn(t, 0.06, 0.2))
                    KeyBubble(text: key("addressbar.commit"), visible: tourOn(t, 0.55, 0.68))
                }
                .frame(width: 70)
            }
            if t >= 0.6 {
                HStack(spacing: 10) {
                    Image(systemName: "folder.badge.plus").font(.title2).foregroundStyle(.tint)
                    VStack(alignment: .leading, spacing: 2) {
                        Text("“new idea” doesn’t exist yet").font(.system(size: 13, weight: .semibold))
                        Text("in ~/My Projects").font(.system(size: 11)).foregroundStyle(.secondary)
                    }
                    Spacer()
                    Text("Cancel").font(.system(size: 12)).padding(.horizontal, 10).frame(height: 24)
                        .background(RoundedRectangle(cornerRadius: 6).fill(Color.primary.opacity(0.08)))
                    Text("Create Folder").font(.system(size: 12, weight: .semibold)).foregroundStyle(.white)
                        .padding(.horizontal, 10).frame(height: 24)
                        .background(RoundedRectangle(cornerRadius: 6).fill(Color.accentColor))
                        .scaleEffect(tourOn(t, 0.85, 0.92) ? 0.94 : 1)
                }
                .padding(12)
                .frame(maxWidth: 440)
                .background(RoundedRectangle(cornerRadius: 10).fill(Color(nsColor: .windowBackgroundColor)).shadow(radius: 5, y: 2))
                .opacity(tourSeg(t, 0.6, 0.66))
            }
            Spacer(minLength: 0)
        }
    }
}

// MARK: - 3. Dual pane

private struct DualPaneDemo: View {
    let t: Double
    let key: (String) -> String

    var body: some View {
        let copied = t >= 0.55
        let rightActive = t >= 0.78
        let fly = tourSeg(t, 0.28, 0.55)
        GeometryReader { g in
            ZStack(alignment: .topLeading) {
                HStack(spacing: 8) {
                    DemoPane(title: "Downloads", active: !rightActive) {
                        DemoRow(icon: "doc.richtext", name: "Invoice.pdf", detail: "88 KB", selected: true, focused: !rightActive)
                        DemoRow(icon: "photo", name: "scan.jpg", detail: "1.2 MB")
                        DemoRow(icon: "doc.zipper", name: "fonts.zip", detail: "6.4 MB")
                    }
                    DemoPane(title: "Accounts 2026", active: rightActive) {
                        DemoRow(icon: "tablecells", name: "Budget.numbers", detail: "220 KB")
                        if copied {
                            DemoRow(icon: "doc.richtext", name: "Invoice.pdf", detail: "88 KB", selected: rightActive, focused: rightActive)
                                .transition(.opacity)
                        }
                        DemoRow(icon: "doc.text", name: "Notes.txt", detail: "4 KB")
                    }
                }
                if tourOn(t, 0.28, 0.55) {
                    DemoRow(icon: "doc.richtext", name: "Invoice.pdf", selected: true)
                        .frame(width: g.size.width / 2 - 20)
                        .shadow(radius: 4, y: 2)
                        .offset(x: 4 + fly * (g.size.width / 2), y: 30 + fly * 22)
                        .opacity(0.9)
                }
                ZStack {
                    KeyBubble(text: key("pane.toggle_dual"), visible: tourOn(t, 0.04, 0.18))
                    KeyBubble(text: key("file.copy_to_other_pane"), visible: tourOn(t, 0.24, 0.42))
                    KeyBubble(text: key("pane.switch"), visible: tourOn(t, 0.74, 0.88))
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .bottomTrailing)
            }
        }
    }
}

// MARK: - 4. Operation queue

private struct QueueDemo: View {
    let t: Double
    let key: (String) -> String

    var body: some View {
        let swap = tourSeg(t, 0.25, 0.4)
        let conflict = tourOn(t, 0.5, 0.86)
        ZStack(alignment: .bottom) {
            VStack(alignment: .leading, spacing: 6) {
                HStack {
                    Text("Operations").font(.system(size: 12, weight: .semibold))
                    Spacer()
                    Text("3 waiting").font(.system(size: 11)).foregroundStyle(.secondary)
                }
                op("doc.on.doc", "Copy 1,204 items to Backup", progress: 0.2 + 0.6 * t, status: "\(Int((0.2 + 0.6 * t) * 100))%", paused: false)
                ZStack(alignment: .top) {
                    op("arrow.right.doc.on.clipboard", "Move Photos to Archive", progress: 0.42, status: "Paused", paused: true)
                        .offset(y: swap * 46)
                    op("doc.on.doc", "Copy Projects to NAS", progress: 0, status: "Waiting", paused: false)
                        .offset(y: 46 - swap * 46)
                }
                .frame(height: 88, alignment: .top)
                if t >= 0.86 {
                    HStack(spacing: 6) {
                        TourKeyCap(text: key("edit.undo"), pressed: true)
                        Text("Undo Copy “Report.pdf”").font(.system(size: 12))
                        Spacer()
                        Text("History: 12 operations").font(.system(size: 11)).foregroundStyle(.secondary)
                    }
                    .transition(.opacity)
                }
                Spacer(minLength: 0)
            }
            .opacity(conflict ? 0.6 : 1)
            if conflict {
                VStack(alignment: .leading, spacing: 8) {
                    HStack(spacing: 8) {
                        Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(.orange)
                        Text("“Report.pdf” already exists in Backup").font(.system(size: 13, weight: .semibold))
                        Spacer()
                        Text("yours: yesterday, 1.1 MB · new: today, 1.3 MB").font(.system(size: 11)).foregroundStyle(.secondary)
                    }
                    HStack(spacing: 6) {
                        Image(systemName: "trash").font(.system(size: 11)).foregroundStyle(.secondary)
                        Text("Replace sends the existing one to the Trash.").font(.system(size: 11)).foregroundStyle(.secondary)
                        Spacer()
                        ForEach(["Skip", "Keep Both"], id: \.self) { b in
                            Text(b).font(.system(size: 12)).padding(.horizontal, 9).frame(height: 22)
                                .background(RoundedRectangle(cornerRadius: 6).fill(Color.primary.opacity(0.08)))
                        }
                        Text("Replace").font(.system(size: 12, weight: .semibold)).foregroundStyle(.white)
                            .padding(.horizontal, 9).frame(height: 22)
                            .background(RoundedRectangle(cornerRadius: 6).fill(Color.accentColor))
                            .scaleEffect(tourOn(t, 0.76, 0.82) ? 0.93 : 1)
                    }
                }
                .padding(12)
                .frame(maxWidth: .infinity)
                .background(RoundedRectangle(cornerRadius: 12).fill(Color(nsColor: .windowBackgroundColor)).shadow(radius: 8, y: 3))
                .transition(.opacity)
            }
        }
    }

    private func op(_ icon: String, _ title: String, progress: Double, status: String, paused: Bool) -> some View {
        HStack(spacing: 8) {
            Image(systemName: icon).font(.system(size: 13)).foregroundStyle(.tint).frame(width: 18)
            VStack(alignment: .leading, spacing: 3) {
                HStack {
                    Text(title).font(.system(size: 12))
                    Spacer()
                    Text(status).font(.system(size: 11).monospacedDigit()).foregroundStyle(paused ? .orange : .secondary)
                }
                ProgressView(value: progress).progressViewStyle(.linear).tint(paused ? .orange : .accentColor)
            }
            Image(systemName: paused ? "play.circle" : "pause.circle").foregroundStyle(.secondary)
            Image(systemName: "line.3.horizontal").font(.system(size: 11)).foregroundStyle(.tertiary)
        }
        .padding(7)
        .frame(height: 42)
        .background(RoundedRectangle(cornerRadius: 8).fill(Color(nsColor: .textBackgroundColor)))
    }
}

// MARK: - 5. Cut and paste to move

private struct CutPasteDemo: View {
    let t: Double
    let key: (String) -> String

    var body: some View {
        let cut = t >= 0.2
        let moved = t >= 0.66
        let rightActive = t >= 0.5
        VStack(spacing: 8) {
            HStack(spacing: 8) {
                DemoPane(title: "Desktop", active: !rightActive) {
                    if !moved {
                        HStack(spacing: 0) {
                            DemoRow(icon: "doc.text.image", name: "plan.key", detail: "8.4 MB", selected: !rightActive,
                                    dimmed: cut, focused: !rightActive)
                            if cut { Image(systemName: "scissors").font(.system(size: 11)).foregroundStyle(.tint).padding(.leading, 4) }
                        }
                    }
                    DemoRow(icon: "photo", name: "screenshot.png", detail: "640 KB")
                }
                DemoPane(title: "Projects", active: rightActive) {
                    DemoRow(icon: "folder.fill", name: "Archive", detail: "--")
                    if moved {
                        DemoRow(icon: "doc.text.image", name: "plan.key", detail: "8.4 MB", selected: true).transition(.opacity)
                    }
                }
            }
            HStack(spacing: 8) {
                if cut && !moved {
                    Image(systemName: "info.circle").foregroundStyle(.secondary)
                    Text("Marked to move. Nothing is deleted until you paste.").font(.system(size: 12)).foregroundStyle(.secondary)
                } else if moved {
                    Image(systemName: "checkmark.circle.fill").foregroundStyle(.green)
                    Text("Moved “plan.key” to Projects").font(.system(size: 12))
                }
                Spacer()
                ZStack {
                    KeyBubble(text: key("edit.cut"), visible: tourOn(t, 0.14, 0.3))
                    KeyBubble(text: key("edit.paste"), visible: tourOn(t, 0.58, 0.74))
                }
            }
            .frame(height: 30)
        }
    }
}

// MARK: - 6. Type to select vs filter

private struct TypeSelectDemo: View {
    let t: Double
    let key: (String) -> String
    private let names: [(String, String)] = [
        ("tablecells", "Budget.numbers"), ("doc.text", "Notes.txt"), ("folder.fill", "Photos"),
        ("doc.richtext", "Report.pdf"), ("folder.fill", "Reports 2025"), ("photo", "Zebra.png"),
    ]

    var body: some View {
        let typed = String("rep".prefix(Int(tourSeg(t, 0.08, 0.36) * 3.999)))
        let selected = typed.isEmpty ? 0 : names.firstIndex { $0.1.lowercased().hasPrefix(typed) } ?? 0
        let filter = String("rep".prefix(Int(tourSeg(t, 0.55, 0.75) * 3.999)))
        let shown = names.filter { filter.isEmpty || $0.1.lowercased().contains(filter) }
        HStack(spacing: 10) {
            VStack(alignment: .leading, spacing: 5) {
                HStack {
                    Label("Type to select", systemImage: "character.cursor.ibeam").font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(.secondary)
                    Spacer()
                    if !typed.isEmpty { TypedChip(text: typed).scaleEffect(0.8) }
                }
                DemoPane(title: "Documents") {
                    ForEach(names.indices, id: \.self) { i in
                        DemoRow(icon: names[i].0, name: names[i].1, selected: i == selected)
                    }
                }
            }
            VStack(alignment: .leading, spacing: 5) {
                HStack {
                    Label("Filter", systemImage: "line.3.horizontal.decrease.circle").font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(.secondary)
                    Spacer()
                    KeyBubble(text: key("filter.focus"), visible: tourOn(t, 0.45, 0.6))
                        .scaleEffect(0.8)
                }
                DemoPane(title: "Documents", active: t >= 0.45) {
                    HStack(spacing: 5) {
                        Image(systemName: "magnifyingglass").font(.system(size: 11)).foregroundStyle(.secondary)
                        Text(filter.isEmpty ? "Filter" : filter).font(.system(size: 12))
                            .foregroundStyle(filter.isEmpty ? .secondary : .primary)
                        Spacer()
                        if !filter.isEmpty {
                            Text("\(shown.count) of \(names.count)").font(.system(size: 10)).foregroundStyle(.secondary)
                        }
                    }
                    .padding(.horizontal, 7).frame(height: 22)
                    .background(RoundedRectangle(cornerRadius: 6).strokeBorder(Color.primary.opacity(0.15)))
                    ForEach(shown.indices, id: \.self) { i in
                        DemoRow(icon: shown[i].0, name: shown[i].1, selected: i == 0 && !filter.isEmpty)
                    }
                }
            }
        }
    }
}

// MARK: - 7. Hold ⌘ for the overlay, and hints

private struct OverlayDemo: View {
    let t: Double
    let key: (String) -> String
    let rows: [(String, String)]

    var body: some View {
        let held = tourOn(t, 0.1, 0.92)
        let overlay = tourOn(t, 0.22, 0.92)
        ZStack(alignment: .topTrailing) {
            VStack(alignment: .leading, spacing: 8) {
                HStack(spacing: 8) {
                    TourKeyCap(text: "⌘", pressed: held)
                    Text(held ? "holding ⌘ …" : "").font(.system(size: 12)).foregroundStyle(.secondary)
                    Spacer()
                }
                DemoPane(title: "Documents", active: true) {
                    DemoRow(icon: "folder.fill", name: "Invoices")
                    DemoRow(icon: "doc.richtext", name: "Report.pdf", selected: true)
                    DemoRow(icon: "doc.text", name: "Notes.txt")
                }
                .frame(maxWidth: 260)
            }
            if overlay {
                VStack(alignment: .leading, spacing: 4) {
                    Text("⌘ Command").font(.system(size: 11, weight: .semibold)).foregroundStyle(.secondary)
                    ForEach(Array(shownRows.enumerated()), id: \.offset) { _, r in
                        HStack(spacing: 8) {
                            Text(r.0).font(.system(size: 11, weight: .semibold, design: .rounded)).frame(width: 38, alignment: .leading)
                            Text(r.1).font(.system(size: 11)).lineLimit(1)
                        }
                    }
                }
                .padding(10)
                .frame(width: 230, alignment: .leading)
                .background(RoundedRectangle(cornerRadius: 10).fill(Color(nsColor: .windowBackgroundColor)).shadow(radius: 6, y: 2))
                .transition(.opacity)
            }
            if t >= 0.6 {
                HStack(spacing: 8) {
                    TourKeyCap(text: key("file.new_folder"))
                    VStack(alignment: .leading, spacing: 1) {
                        Label("Did you know?", systemImage: "lightbulb.fill").font(.system(size: 10, weight: .semibold))
                            .foregroundStyle(.orange)
                        Text("Press \(key("file.new_folder")) for New Folder").font(.system(size: 11))
                    }
                }
                .padding(9)
                .background(RoundedRectangle(cornerRadius: 10).fill(Color(nsColor: .windowBackgroundColor)).shadow(radius: 6, y: 2))
                .opacity(tourSeg(t, 0.6, 0.66))
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .bottomTrailing)
            }
        }
    }

    private var shownRows: [(String, String)] {
        rows.isEmpty ? [("⌘C", "Copy"), ("⌘V", "Paste"), ("⌘D", "Duplicate"), ("⌘I", "Get Info"), ("⌘L", "Go to Address Bar")]
            : Array(rows.prefix(5))
    }
}

// MARK: - 8. Everything is rebindable

private struct RebindDemo: View {
    let t: Double
    let key: (String) -> String

    var body: some View {
        let recording = tourOn(t, 0.25, 0.5)
        let rebound = tourOn(t, 0.5, 0.95)
        let vim = t >= 0.72
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 0) {
                ForEach(["Standard", "Vim"], id: \.self) { p in
                    let on = (p == "Vim") == vim
                    Text(p).font(.system(size: 12, weight: on ? .semibold : .regular))
                        .padding(.horizontal, 14).frame(height: 24)
                        .background(RoundedRectangle(cornerRadius: 6).fill(on ? Color.accentColor.opacity(0.22) : .clear))
                }
            }
            .padding(2)
            .background(RoundedRectangle(cornerRadius: 8).fill(Color.primary.opacity(0.06)))
            VStack(spacing: 0) {
                row("Copy to Other Pane", cap: rebound ? "⌃⌥C" : key("file.copy_to_other_pane"), recording: recording, highlight: true)
                Divider()
                row("Move to Other Pane", cap: key("file.move_to_other_pane"), recording: false, highlight: false)
                Divider()
                row("Toggle Dual Pane", cap: key("pane.toggle_dual"), recording: false, highlight: false)
            }
            .background(RoundedRectangle(cornerRadius: 8).fill(Color(nsColor: .textBackgroundColor)))
            .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(Color.primary.opacity(0.1)))
            if vim {
                HStack(spacing: 6) {
                    ForEach(["j k", "y y", "p", "d d", "/"], id: \.self) { TourKeyCap(text: $0) }
                    Text("Vim keys in the list").font(.system(size: 11)).foregroundStyle(.secondary)
                }
                .transition(.opacity)
            }
            Spacer(minLength: 0)
        }
        .frame(maxWidth: 460)
    }

    private func row(_ title: String, cap: String, recording: Bool, highlight: Bool) -> some View {
        HStack {
            Text(title).font(.system(size: 12))
            Spacer()
            if recording {
                Text("Type a shortcut…").font(.system(size: 11)).foregroundStyle(.tint)
                    .padding(.horizontal, 8).frame(height: 24)
                    .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(Color.accentColor, lineWidth: 1.5))
            } else {
                TourKeyCap(text: cap, pressed: highlight && tourOn(t, 0.5, 0.6))
            }
        }
        .padding(.horizontal, 10)
        .frame(height: 34)
    }
}

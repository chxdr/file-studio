// SetupView.swift — first-run setup (F-011): Welcome → Full Disk Access →
// Keys → Layout → zoxide history (only when a database is found) → Done.
// Short and skippable. Motion follows Import Studio's D-026: spring page
// transitions in the direction of travel, a sliding step pill with
// checkmarks, staggered entrances; fades only under Reduce Motion.
// Choices are written to core settings as they are made.

import Combine
import SwiftUI

struct SetupView: View {
    let model: AppModel
    let state: SetupState
    let finish: () -> Void
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Namespace private var stepPill

    var body: some View {
        VStack(spacing: 0) {
            stepIndicator
                .padding(.horizontal, 20)
                .padding(.top, 18)
                .padding(.bottom, 14)
            Divider()
            ZStack {
                page
                    .id(state.index)
                    .transition(pageTransition)
                    .padding(28)
                    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            }
            .clipped()
            Divider()
            HStack {
                if state.index == 0 {
                    Button("Skip Setup") { finish() }
                        .help("You can run setup again from Settings")
                } else if !state.isLast {
                    Button { SetupController.go(to: state.index - 1) } label: { Label("Back", systemImage: "chevron.left") }
                }
                Spacer()
                if !state.isLast && state.index > 0 {
                    Button("Skip Setup") { finish() }.buttonStyle(.borderless).foregroundStyle(.secondary)
                }
                Button {
                    if state.isLast { finish() } else { SetupController.go(to: state.index + 1) }
                } label: {
                    HStack(spacing: 6) {
                        Text(state.isLast ? "Start Browsing" : (state.index == 0 ? "Get Started" : "Continue"))
                        Image(systemName: state.isLast ? "arrow.right.circle.fill" : "chevron.right")
                            .contentTransition(.symbolEffect(.replace))
                    }
                }
                .keyboardShortcut(.defaultAction)
                .buttonStyle(.borderedProminent)
                .controlSize(.large)
            }
            .padding(16)
        }
        .frame(width: 660, height: 540)
    }

    // MARK: Step indicator

    private var stepIndicator: some View {
        let titles = state.titles
        return HStack(spacing: 4) {
            ForEach(titles.indices, id: \.self) { i in
                HStack(spacing: 5) {
                    ZStack {
                        Circle().fill(i < state.index ? Color.accentColor : Color.secondary.opacity(0.18))
                            .frame(width: 18, height: 18)
                        if i < state.index {
                            Image(systemName: "checkmark")
                                .font(.system(size: 9, weight: .bold))
                                .foregroundStyle(.white)
                                .transition(.scale.combined(with: .opacity))
                        } else {
                            Text("\(i + 1)").font(.system(size: 10, weight: .semibold))
                                .foregroundStyle(i == state.index ? .primary : .secondary)
                        }
                    }
                    Text(titles[i])
                        .font(.callout.weight(i == state.index ? .semibold : .regular))
                        .foregroundStyle(i == state.index ? .primary : .secondary)
                        .lineLimit(1)
                        .fixedSize()
                }
                .padding(.horizontal, 8)
                .padding(.vertical, 5)
                .background {
                    if i == state.index {
                        Capsule().fill(Color.accentColor.opacity(0.16))
                            .matchedGeometryEffect(id: "step", in: stepPill)
                    }
                }
                .accessibilityElement(children: .ignore)
                .accessibilityLabel("Step \(i + 1) of \(titles.count), \(titles[i])"
                                    + (i < state.index ? ", done" : i == state.index ? ", current" : ""))
                if i < titles.count - 1 {
                    Capsule()
                        .fill(i < state.index ? Color.accentColor.opacity(0.7) : Color.secondary.opacity(0.2))
                        .frame(width: 10, height: 2)
                        .accessibilityHidden(true)
                }
            }
        }
        .frame(maxWidth: .infinity)
    }

    private var pageTransition: AnyTransition {
        guard !reduceMotion else { return .opacity }
        return .asymmetric(
            insertion: .move(edge: state.forward ? .trailing : .leading).combined(with: .opacity),
            removal: .move(edge: state.forward ? .leading : .trailing).combined(with: .opacity))
    }

    // MARK: Pages

    @ViewBuilder private var page: some View {
        switch state.page {
        case .welcome: WelcomePage()
        case .fullDiskAccess: FullDiskAccessPage(state: state)
        case .keymap: KeymapPage(model: model)
        case .layout: LayoutPage(model: model)
        case .zoxide: ZoxidePage(model: model)
        case .reread: RereadPage(model: model)
        case .done: DonePage(model: model)
        }
    }
}

/// Title and subtitle that rise in, then the step's content.
private struct StepPage<Content: View>: View {
    let title: String
    let subtitle: String
    @ViewBuilder let content: Content
    @State private var shown = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text(title).font(.title.bold())
                .opacity(shown ? 1 : 0).offset(y: shown || reduceMotion ? 0 : 8)
                .accessibilityAddTraits(.isHeader)
            Text(subtitle).font(.callout).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                .opacity(shown ? 1 : 0).offset(y: shown || reduceMotion ? 0 : 8)
            content
                .padding(.top, 6)
                .opacity(shown ? 1 : 0)
        }
        .onAppear { withAnimation(.easeOut(duration: 0.3).delay(0.08)) { shown = true } }
    }
}

private struct WelcomePage: View {
    @State private var shown = 0
    @State private var bounce = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private let rows: [(String, String, String)] = [
        ("rectangle.split.2x1", "Two panes, many tabs", "browse side by side, drag tabs between panes."),
        ("character.cursor.ibeam", "An address bar that completes", "type or paste any path; it learns where you go."),
        ("keyboard", "Every key is yours", "Finder’s shortcuts by default, Vim if you like, all rebindable."),
    ]

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            ZStack {
                Circle()
                    .fill(LinearGradient(colors: [Color.accentColor.opacity(0.35), Color.accentColor.opacity(0.08)],
                                         startPoint: .topLeading, endPoint: .bottomTrailing))
                    .frame(width: 88, height: 88)
                    .scaleEffect(shown > 0 || reduceMotion ? 1 : 0.6)
                Image(systemName: "folder.fill")
                    .font(.system(size: 40))
                    .foregroundStyle(.tint)
                    .symbolEffect(.bounce, value: bounce)
            }
            .opacity(shown > 0 ? 1 : 0)
            .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 6) {
                Text("Welcome to File Studio").font(.largeTitle.bold()).accessibilityAddTraits(.isHeader)
                Text("A fast file manager for the keyboard and the mouse. Four quick choices and you’re in.")
                    .font(.title3).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .opacity(shown > 1 ? 1 : 0)
            .offset(y: shown > 1 || reduceMotion ? 0 : 10)
            VStack(alignment: .leading, spacing: 12) {
                ForEach(rows.indices, id: \.self) { i in
                    HStack(alignment: .top, spacing: 12) {
                        Image(systemName: rows[i].0).font(.title3).foregroundStyle(.tint).frame(width: 28)
                            .accessibilityHidden(true)
                        Text("\(Text(rows[i].1).bold()): \(rows[i].2)")
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    .opacity(shown > i + 2 ? 1 : 0)
                    .offset(x: shown > i + 2 || reduceMotion ? 0 : 18)
                }
            }
        }
        .task {
            for i in 1...5 {
                try? await Task.sleep(for: .milliseconds(reduceMotion ? 0 : (i == 1 ? 60 : 90)))
                withAnimation(reduceMotion ? nil : .spring(duration: 0.45, bounce: 0.25)) { shown = i }
            }
            bounce.toggle()
        }
    }
}

private struct FullDiskAccessPage: View {
    let state: SetupState
    private let timer = Timer.publish(every: 1, on: .main, in: .common).autoconnect()

    var body: some View {
        StepPage(title: "Let File Studio see every folder",
                 subtitle: "macOS keeps some folders (Mail, Messages, Safari, other apps’ data, Time Machine) away from apps until you grant Full Disk Access. Without it, those folders show as empty or locked. File Studio never uploads anything.") {
            VStack(alignment: .leading, spacing: 16) {
                HStack(spacing: 12) {
                    Image(systemName: state.hasFullDiskAccess ? "checkmark.shield.fill" : "lock.shield")
                        .font(.system(size: 30))
                        .foregroundStyle(state.hasFullDiskAccess ? Color.green : Color.secondary)
                        .contentTransition(.symbolEffect(.replace))
                        .accessibilityHidden(true)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(state.hasFullDiskAccess ? "Full Disk Access is on" : "Full Disk Access is off")
                            .font(.headline)
                        Text(state.hasFullDiskAccess ? "Every folder is reachable."
                             : "Optional. You can grant it later in System Settings.")
                            .foregroundStyle(.secondary)
                    }
                }
                .accessibilityElement(children: .combine)
                if !state.hasFullDiskAccess {
                    Button {
                        SetupController.openFullDiskAccessSettings()
                    } label: {
                        Label("Open Privacy & Security…", systemImage: "gear")
                    }
                    .controlSize(.large)
                    Text("In System Settings, turn on File Studio in the list (use + if it isn’t there). This page notices when it’s on.")
                        .font(.callout).foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
        }
        .onReceive(timer) { _ in
            let now = SetupController.hasFullDiskAccess()
            if now != state.hasFullDiskAccess { withAnimation(Motion.fade) { state.hasFullDiskAccess = now } }
        }
    }
}

/// A large selectable card (keymap and layout choices).
private struct ChoiceCard<Art: View>: View {
    let title: String
    let detail: String
    let selected: Bool
    let action: () -> Void
    @ViewBuilder let art: Art
    @State private var hover = false

    var body: some View {
        Button(action: action) {
            VStack(alignment: .leading, spacing: 10) {
                art.frame(maxWidth: .infinity).frame(height: 96)
                HStack {
                    Text(title).font(.headline)
                    Spacer()
                    Image(systemName: selected ? "checkmark.circle.fill" : "circle")
                        .foregroundStyle(selected ? Color.accentColor : Color.secondary)
                        .contentTransition(.symbolEffect(.replace))
                }
                Text(detail).font(.callout).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                Spacer(minLength: 0)
            }
            .padding(14)
            .frame(maxWidth: .infinity, minHeight: 220, alignment: .topLeading)
            .background(RoundedRectangle(cornerRadius: 12, style: .continuous)
                .fill(selected ? Color.accentColor.opacity(0.12) : Color.primary.opacity(hover ? 0.06 : 0.03)))
            .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous)
                .strokeBorder(selected ? Color.accentColor : Color.primary.opacity(0.12), lineWidth: selected ? 2 : 1))
            .contentShape(RoundedRectangle(cornerRadius: 12))
        }
        .buttonStyle(.plain)
        .onHover { h in withAnimation(Motion.fade) { hover = h } }
        .accessibilityElement(children: .combine)
        .accessibilityAddTraits(selected ? [.isButton, .isSelected] : .isButton)
    }
}

private struct KeymapPage: View {
    let model: AppModel

    var body: some View {
        let profile = model.settings.keyboard.profile
        StepPage(title: "How do you like your keys?",
                 subtitle: "Standard works like Finder. Vim adds modal keys on top (hjkl, gg, G, visual mode, marks) without taking Finder’s shortcuts away.") {
            HStack(spacing: 14) {
                ChoiceCard(title: "Standard", detail: "Finder’s shortcuts: ⌘↓ opens, ⌘[ goes back, type to select.",
                           selected: profile == .standard, action: { set(.standard) }) {
                    keys(["⌘", "↓"], ["⌘", "["], ["⌘", "T"])
                }
                ChoiceCard(title: "Vim on top", detail: "Everything in Standard, plus j/k to move, gg/G, / to filter, v to select.",
                           selected: profile == .vim, action: { set(.vim) }) {
                    keys(["h", "j", "k", "l"], ["g", "g"], ["v"])
                }
            }
        }
    }

    private func keys(_ groups: [String]...) -> some View {
        VStack(spacing: 8) {
            ForEach(groups.indices, id: \.self) { g in
                HStack(spacing: 4) {
                    ForEach(groups[g].indices, id: \.self) { k in
                        Text(groups[g][k])
                            .font(.system(size: 13, weight: .medium, design: .rounded))
                            .frame(minWidth: 24, minHeight: 22)
                            .background(RoundedRectangle(cornerRadius: 5).fill(Color.primary.opacity(0.08)))
                            .overlay(RoundedRectangle(cornerRadius: 5).strokeBorder(Color.primary.opacity(0.15)))
                    }
                }
            }
        }
        .accessibilityHidden(true)
    }

    private func set(_ p: Profile) {
        var s = model.settings
        guard s.keyboard.profile != p else { return }
        s.keyboard.profile = p
        do { try model.engine.updateSettings(s) } catch { model.statusMessage = error.localizedDescription }
    }
}

private struct LayoutPage: View {
    let model: AppModel

    var body: some View {
        let dual = model.settings.general.dualPaneByDefault
        StepPage(title: "One pane or two?",
                 subtitle: "New windows open this way. Either way, ⌘\\ switches at any time, and each window remembers its layout.") {
            HStack(spacing: 14) {
                ChoiceCard(title: "Single pane", detail: "One folder at a time, with tabs. Calm and familiar.",
                           selected: !dual, action: { set(false) }) {
                    diagram(panes: 1)
                }
                ChoiceCard(title: "Dual pane", detail: "Two folders side by side: copy and move between them in one key.",
                           selected: dual, action: { set(true) }) {
                    diagram(panes: 2)
                }
            }
        }
    }

    /// The window as it will open: the app's own panes (TourDemoParts), shrunk.
    private func diagram(panes: Int) -> some View {
        let cols: [(ColumnID, CGFloat)] = panes == 2 ? [(.name, 0), (.size, 56)] : [(.name, 0), (.size, 70), (.modified, 140)]
        let docs = [("Design", "--"), ("Budget.numbers", "220 KB"), ("Notes.txt", "4 KB"), ("Report.pdf", "1.3 MB")]
        let left = DemoPaneSpec(crumbs: TourDemoData.crumbs(["Documents"]), bar: .rest(TourDemoData.crumbs(["Documents"])),
                                subtitle: TourDemoData.subtitle(4),
                                rows: docs.map { TourDemoData.row($0.0, folder: $0.1 == "--", size: $0.1,
                                                                  selected: $0.0 == "Report.pdf") },
                                columns: cols, marked: panes == 2, status: PaneText.items(4))
        let right = DemoPaneSpec(crumbs: TourDemoData.crumbs(["Downloads"]), bar: .rest(TourDemoData.crumbs(["Downloads"])),
                                 subtitle: TourDemoData.subtitle(2),
                                 rows: [TourDemoData.row("fonts.zip", size: "6.4 MB"), TourDemoData.row("scan.jpg", size: "1.2 MB")],
                                 columns: cols, focused: false, status: PaneText.items(2))
        return DemoWindow(panes: panes == 2 ? [left, right] : [left])
            .frame(width: 400, height: 210)
            .demoWindowFrame()
            .scaleEffect(0.45)
            .frame(width: 180, height: 95)
            .allowsHitTesting(false)
            .accessibilityHidden(true)
    }

    private func set(_ dual: Bool) {
        var s = model.settings
        guard s.general.dualPaneByDefault != dual else { return }
        s.general.dualPaneByDefault = dual
        do {
            try model.engine.updateSettings(s)
            // Show the choice in the window behind the sheet too.
            if let w = model.keyWindowID, let wm = model.windowModel(w), wm.isDual != dual {
                _ = try model.engine.toggleDual(window: w)
            }
        } catch { model.statusMessage = error.localizedDescription }
    }
}

private struct ZoxidePage: View {
    let model: AppModel
    @State private var result: String?
    @State private var failed = false

    var body: some View {
        StepPage(title: "Bring your zoxide history",
                 subtitle: "zoxide is installed here. File Studio can import the folders it remembers once, so the address bar suggests them from day one. zoxide itself is left untouched.") {
            VStack(alignment: .leading, spacing: 14) {
                if let result {
                    Label(result, systemImage: failed ? "exclamationmark.triangle.fill" : "checkmark.circle.fill")
                        .foregroundStyle(failed ? Color.orange : Color.green)
                        .font(.headline)
                        .transition(.scale(scale: 0.9).combined(with: .opacity))
                } else {
                    Button {
                        importNow()
                    } label: {
                        Label("Import zoxide History", systemImage: "square.and.arrow.down")
                    }
                    .controlSize(.large)
                    Text("Or press Continue to skip; you can import later with `fstudio import-zoxide`.")
                        .font(.callout).foregroundStyle(.secondary)
                }
            }
        }
    }

    private func importNow() {
        do {
            let r = try model.engine.importZoxide(dbPath: nil)
            withAnimation(.spring(duration: 0.4, bounce: 0.3)) {
                result = "Imported \(r.added) folders" + (r.merged > 0 ? " (\(r.merged) already known)" : "")
            }
        } catch {
            failed = true
            withAnimation(Motion.fade) { result = error.localizedDescription }
        }
    }
}

/// "Read originals twice?" (Safe Transfer proposal §4.6): opt-in, off by
/// default; the choice is `operations.reread_originals`.
private struct RereadPage: View {
    let model: AppModel
    @State private var chosen: Bool?

    var body: some View {
        StepPage(title: "Read originals twice?",
                 subtitle: "File Studio always checks every copy against the original before giving it its real name. That catches a copy that went wrong on the way to the disk.") {
            VStack(alignment: .leading, spacing: 10) {
                Text("It can’t catch a copy that was wrong from the start: if a failing card or card reader hands over a bad byte while the original is being read, the copy matches that bad byte perfectly.")
                    .font(.callout).fixedSize(horizontal: false, vertical: true)
                Text("**Turn this on to read each original a second time** and make sure both reads agree. If they don’t, the file is flagged and your original is left untouched.")
                    .font(.callout).fixedSize(horizontal: false, vertical: true)
                VStack(alignment: .leading, spacing: 2) {
                    Text("Who it’s for:").font(.callout.bold())
                    Text("• copying straight off camera cards that have shown errors")
                    Text("• ageing or unreliable card readers")
                    Text("• archives and client deliveries, where a second read is worth the time")
                }
                .font(.callout)
                Text("**The cost:** copies take longer, about twice the reading time from the original’s disk. From a card, a 30 GB job takes about 3 minutes more.")
                    .font(.callout).fixedSize(horizontal: false, vertical: true)
                HStack(spacing: 10) {
                    Button("Read originals twice") { set(true) }
                        .buttonStyle(.borderedProminent)
                    Button("Not now") { set(false) }
                    if let chosen {
                        Label(chosen ? "On" : "Off", systemImage: "checkmark.circle.fill")
                            .foregroundStyle(.green)
                    }
                }
                .controlSize(.large)
                Text("You can change this any time in Settings › Operations.")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
    }

    private func set(_ on: Bool) {
        do {
            try Core.set(model.engine, "operations.reread_originals", bool: on)
            chosen = on
        } catch { model.statusMessage = error.localizedDescription }
    }
}

private struct DonePage: View {
    let model: AppModel
    @State private var shown = false
    @State private var bounce = false
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    var body: some View {
        VStack(spacing: 16) {
            Spacer()
            Image(systemName: "checkmark.circle.fill")
                .font(.system(size: 72))
                .foregroundStyle(.green)
                .symbolEffect(.bounce, value: bounce)
                .scaleEffect(shown || reduceMotion ? 1 : 0.4)
                .opacity(shown ? 1 : 0)
                .accessibilityHidden(true)
            Text("You’re all set").font(.largeTitle.bold()).opacity(shown ? 1 : 0)
            Text(summary)
                .font(.title3).foregroundStyle(.secondary).multilineTextAlignment(.center)
                .opacity(shown ? 1 : 0)
            Text("Hold ⌘ for a moment to see what you can press. Everything here is in Settings.")
                .foregroundStyle(.secondary).multilineTextAlignment(.center)
                .opacity(shown ? 1 : 0)
            // The optional power tour (also in Help › File Studio Power Tour).
            Button {
                SetupController.finish(model: model)
                DispatchQueue.main.async { MainActor.assumeIsolated { TourController.present(model: model) } }
            } label: {
                Label("Take the 1-minute tour", systemImage: "play.circle.fill")
            }
            .controlSize(.large)
            .padding(.top, 4)
            .opacity(shown ? 1 : 0)
            .help("Eight cards on the power-user tools and their keys. Also in Help › File Studio Power Tour.")
            Spacer()
        }
        .frame(maxWidth: .infinity)
        .onAppear {
            withAnimation(reduceMotion ? .easeIn(duration: 0.15) : .spring(duration: 0.5, bounce: 0.35).delay(0.1)) { shown = true }
            Task { try? await Task.sleep(for: .milliseconds(400)); bounce.toggle() }
        }
    }

    private var summary: String {
        let s = model.settings
        return (s.keyboard.profile == .vim ? "Vim keys" : "Standard keys") + " · "
            + (s.general.dualPaneByDefault ? "Dual pane" : "Single pane")
    }
}

// SettingsShortcuts.swift — the Shortcuts tab (Nimble Commander's hotkeys
// pane as the model): a searchable table of every command with a primary and
// a secondary key (click to record a chord), a Conflicts list, a Customised
// list, export/import as JSON, and the keyboard, overlay and hint settings.
// What a key means and which keys are refused (system shortcuts, clashes) is
// the core's call; this only records chords and shows the answers.

import AppKit
import SwiftUI
import UniformTypeIdentifiers

enum ShortcutsSection: String, CaseIterable {
    case commands = "Commands", conflicts = "Conflicts", customised = "Customised", behaviour = "Keyboard & Hints"
}

struct SettingsShortcutsPane: View {
    let model: AppModel
    @State private var section: ShortcutsSection = .commands
    @State private var query = ""
    @State private var commands: [CommandInfo] = []
    @State private var keys: [String: [String]] = [:]
    @State private var conflicts: [String] = []
    @State private var recorder = ChordRecorder()
    @State private var error: String?

    var body: some View {
        VStack(spacing: 10) {
            Picker("", selection: $section) {
                ForEach(ShortcutsSection.allCases, id: \.self) { s in
                    Text(label(s)).tag(s)
                }
            }
            .pickerStyle(.segmented).labelsHidden()
            .padding(.horizontal, 16).padding(.top, 12)
            switch section {
            case .commands: table(commands.filter(matches))
            case .conflicts: conflictsList
            case .customised: table(commands.filter { $0.customized && matches($0) }, empty: "You haven't changed any shortcuts.")
            case .behaviour: SettingsKeyboardForm(model: model)
            }
        }
        .onAppear(perform: reload)
        .onChange(of: model.settings) { reload() }
        .onDisappear { recorder.stop() }
        #if DEBUG
        .onReceive(NotificationCenter.default.publisher(for: .fsDebugRecord)) { n in
            guard let id = n.object as? String else { return }
            if id.hasPrefix("section:") {
                section = ShortcutsSection.allCases.first { $0.rawValue.lowercased().hasPrefix(id.dropFirst(8).lowercased()) } ?? .commands
            } else if let c = commands.first(where: { $0.id == id }) {
                section = .commands
                record(c, primary: true)
            }
        }
        #endif
    }

    private func label(_ s: ShortcutsSection) -> String {
        switch s {
        case .conflicts:
            return conflicts.isEmpty ? s.rawValue : "\(s.rawValue) (\(conflicts.count))"
        case .customised:
            let n = commands.filter(\.customized).count
            return n == 0 ? s.rawValue : "\(s.rawValue) (\(n))"
        default:
            return s.rawValue
        }
    }

    private func reload() {
        commands = model.engine.commands().filter { $0.handling != .later }
        keys = Dictionary(uniqueKeysWithValues: commands.map { ($0.id, model.engine.commandKeys($0.id)) })
        conflicts = model.engine.keymapConflicts()
    }

    private func matches(_ c: CommandInfo) -> Bool {
        let q = query.trimmingCharacters(in: .whitespaces).lowercased()
        guard !q.isEmpty else { return true }
        return c.title.lowercased().contains(q) || c.id.lowercased().contains(q) || c.category.lowercased().contains(q)
            || (keys[c.id] ?? []).contains { $0.lowercased().contains(q) }
    }

    // MARK: Table

    private func table(_ rows: [CommandInfo], empty: String = "No commands match.") -> some View {
        VStack(spacing: 0) {
            HStack {
                Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
                TextField("Search commands, categories or keys", text: $query).textFieldStyle(.plain)
                if !query.isEmpty { Button { query = "" } label: { Image(systemName: "xmark.circle.fill") }.buttonStyle(.plain).foregroundStyle(.secondary) }
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
            if rows.isEmpty {
                Text(empty).foregroundStyle(.secondary).frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                ScrollView {
                    LazyVStack(spacing: 0) {
                        ForEach(rows) { c in
                            row(c)
                            Divider().padding(.leading, 24)
                        }
                    }
                }
            }
            if let error {
                Label(error, systemImage: "exclamationmark.triangle.fill").font(.callout).foregroundStyle(.red)
                    .frame(maxWidth: .infinity, alignment: .leading).padding(10)
                    .background(Color.red.opacity(0.08))
            }
            Divider()
            HStack {
                Button("Export…", action: exportKeys)
                Button("Import…", action: importKeys)
                Spacer()
                Button("Reset All to Defaults") { run { try model.engine.resetCommandKeys(nil) } }
                    .disabled(!commands.contains(where: \.customized))
            }
            .padding(10)
        }
    }

    private func row(_ c: CommandInfo) -> some View {
        let k = keys[c.id] ?? []
        return HStack {
            VStack(alignment: .leading, spacing: 1) {
                Text(c.title).lineLimit(1)
                Text(c.category).font(.caption).foregroundStyle(.secondary)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            KeySlot(text: k.first, recording: recorder.active == .init(id: c.id, primary: true), customised: c.customized) {
                record(c, primary: true)
            } clear: { set(c, primary: true, nil) }
            .frame(width: 130)
            KeySlot(text: k.count > 1 ? k[1] : nil, recording: recorder.active == .init(id: c.id, primary: false), customised: false) {
                record(c, primary: false)
            } clear: { set(c, primary: false, nil) }
            .frame(width: 130)
            Button { run { try model.engine.resetCommandKeys(c.id) } } label: { Image(systemName: "arrow.uturn.backward") }
                .buttonStyle(.borderless).help("Reset to default")
                .opacity(c.customized ? 1 : 0).disabled(!c.customized).frame(width: 28)
        }
        .padding(.horizontal, 24).padding(.vertical, 5)
        .help(c.description)
    }

    private var conflictsList: some View {
        Group {
            if conflicts.isEmpty {
                VStack(spacing: 8) {
                    Image(systemName: "checkmark.circle").font(.system(size: 30)).foregroundStyle(.green)
                    Text("No conflicts").font(.headline)
                    Text("Every shortcut does one thing.").foregroundStyle(.secondary)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                List(conflicts, id: \.self) { Label($0, systemImage: "exclamationmark.triangle.fill").foregroundStyle(.primary) }
            }
        }
    }

    // MARK: Editing

    private func record(_ c: CommandInfo, primary: Bool) {
        error = nil
        recorder.start(.init(id: c.id, primary: primary)) { chord in
            guard let chord else { return }
            set(c, primary: primary, KeyCapText.text(chord))
        }
    }

    private func set(_ c: CommandInfo, primary: Bool, _ text: String?) {
        run { try model.engine.setCommandKey(c.id, primary: primary, keys: text) }
    }

    private func run(_ f: () throws -> Void) {
        do { try f(); error = nil } catch {
            self.error = error.localizedDescription.replacingOccurrences(of: "invalid setting: ", with: "")
        }
        reload()
    }

    // MARK: Export / import

    private func exportKeys() {
        let custom = commands.filter(\.customized)
        let doc: [String: Any] = ["profile": model.settings.keyboard.profile == .vim ? "Vim" : "Standard",
                                  "shortcuts": Dictionary(uniqueKeysWithValues: custom.map { ($0.id, keys[$0.id] ?? []) })]
        guard let data = try? JSONSerialization.data(withJSONObject: doc, options: [.prettyPrinted, .sortedKeys]) else { return }
        let p = NSSavePanel()
        p.allowedContentTypes = [.json]
        p.nameFieldStringValue = "File Studio Shortcuts.json"
        guard p.runModal() == .OK, let url = p.url else { return }
        do { try data.write(to: url) } catch { self.error = error.localizedDescription }
    }

    private func importKeys() {
        let p = NSOpenPanel()
        p.allowedContentTypes = [.json]
        guard p.runModal() == .OK, let url = p.url else { return }
        do {
            let data = try Data(contentsOf: url)
            guard let doc = try JSONSerialization.jsonObject(with: data) as? [String: Any],
                  let map = doc["shortcuts"] as? [String: [String]] else {
                throw CoreError(.failed, "That file isn't a File Studio shortcuts export.")
            }
            var failed: [String] = []
            for (id, ks) in map {
                do {
                    try model.engine.setCommandKey(id, primary: true, keys: ks.first)
                    try model.engine.setCommandKey(id, primary: false, keys: ks.count > 1 ? ks[1] : nil)
                } catch { failed.append("\(id): \(error.localizedDescription)") }
            }
            error = failed.isEmpty ? nil : failed.joined(separator: "\n")
            if failed.isEmpty { ToastCenter.of(model).show("Imported \(map.count) shortcuts", style: .success) }
        } catch { self.error = error.localizedDescription }
        reload()
    }
}

// MARK: - Key slot

/// One key cell of the Shortcuts table (also drawn by the power tour's demo).
struct KeySlot: View {
    let text: String?
    let recording: Bool
    let customised: Bool
    let action: () -> Void
    let clear: () -> Void
    @State private var hover = false

    var body: some View {
        Button(action: action) {
            HStack(spacing: 4) {
                if recording {
                    Text("Press keys…").foregroundStyle(Color.accentColor)
                } else if let text {
                    Text(text).font(.system(size: 12, weight: .medium, design: .rounded))
                        .foregroundStyle(customised ? Color.accentColor : .primary)
                } else {
                    Text("—").foregroundStyle(.tertiary)
                }
                if text != nil, !recording, hover {
                    Image(systemName: "xmark.circle.fill").foregroundStyle(.secondary)
                        .onTapGesture(perform: clear)
                }
            }
            .padding(.horizontal, 8).frame(maxWidth: .infinity, minHeight: 24)
            .background(RoundedRectangle(cornerRadius: 6).fill(Color.primary.opacity(recording ? 0.14 : 0.06)))
            .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(recording ? Color.accentColor : Color.primary.opacity(0.1)))
        }
        .buttonStyle(.plain)
        .onHover { hover = $0 }
        .help("Click, then press the keys you want")
    }
}

// MARK: - Recording

struct RecorderTarget: Equatable { var id: String; var primary: Bool }

/// Captures the next key chord while a slot is armed. Esc cancels.
@MainActor
@Observable
final class ChordRecorder {
    var active: RecorderTarget?
    @ObservationIgnored private var monitor: Any?

    func start(_ target: RecorderTarget, done: @escaping (KeyChord?) -> Void) {
        stop()
        active = target
        monitor = NSEvent.addLocalMonitorForEvents(matching: [.keyDown]) { [weak self] e in
            guard let self, self.active != nil else { return e }
            let chord = KeyChord(event: e)
            self.stop()
            if let chord, chord.key == .escape, chord.mods.isEmpty { return nil }
            done(chord)
            return nil
        }
    }

    func stop() {
        if let m = monitor { NSEvent.removeMonitor(m) }
        monitor = nil
        active = nil
    }
}

/// A `KeyChord` as key caps (`⇧⌘N`), the form the core's keymap takes.
enum KeyCapText {
    static func text(_ c: KeyChord) -> String {
        var s = ""
        if c.mods.ctrl { s += "⌃" }
        if c.mods.alt { s += "⌥" }
        if c.mods.shift { s += "⇧" }
        if c.mods.cmd { s += "⌘" }
        switch c.key {
        case .char(let ch): s += String(ch).uppercased()
        case .left: s += "←"
        case .right: s += "→"
        case .up: s += "↑"
        case .down: s += "↓"
        case .space: s += "Space"
        case .returnKey, .enter: s += "↩"
        case .escape: s += "⎋"
        case .tab: s += "⇥"
        case .delete: s += "⌫"
        case .forwardDelete: s += "⌦"
        case .home: s += "↖"
        case .end: s += "↘"
        case .pageUp: s += "⇞"
        case .pageDown: s += "⇟"
        case .f(let n): s += "F\(n)"
        }
        return s
    }
}

// MARK: - Keyboard, overlay and hints

struct SettingsKeyboardForm: View {
    let model: AppModel
    private var a: SettingsAccess { SettingsAccess(model: model) }

    var body: some View {
        Form {
            Section("Keyboard") {
                Picker("Shortcut profile", selection: a.binding(\.keyboard.profile)) {
                    Text("Standard").tag(Profile.standard)
                    Text("Vim").tag(Profile.vim)
                }
                SettingsSlider(title: "Key sequence timeout", value: a.double(\.keyboard.sequenceTimeoutMs),
                               range: 200...5000, step: 100) { String(format: "%.1f s", $0 / 1000) }
                Picker("Typing in a list", selection: a.binding(\.keyboard.typingFilters)) {
                    Text("Selects the matching item").tag(false)
                    Text("Filters the list").tag(true)
                }
                Picker("F1–F12", selection: a.binding(\.keyboard.fnKeysAlways)) {
                    Text("As the system is set").tag(false)
                    Text("Always as function keys").tag(true)
                }
            }
            Section("Shortcut overlay") {
                Toggle("Show shortcuts while holding a modifier key", isOn: a.binding(\.overlay.enabled))
                SettingsSlider(title: "Appear after", value: a.double(\.overlay.delayMs), range: 0...2000, step: 50) {
                    $0 == 0 ? "at once" : String(format: "%.2f s", $0 / 1000)
                }
                .disabled(!model.settings.overlay.enabled)
                Picker("Corner", selection: a.binding(\.overlay.corner)) {
                    ForEach(Corner.allCases, id: \.self) { Text($0.title).tag($0) }
                }
                .disabled(!model.settings.overlay.enabled)
                Toggle("Holding Fn lists the keys that need no modifier", isOn: a.binding(\.overlay.listPlainKeys))
                    .disabled(!model.settings.overlay.enabled)
            }
            Section("“Did you know?” hints") {
                Toggle("Suggest the shortcut after using the mouse or a menu", isOn: a.binding(\.hints.enabled))
                Picker("How often", selection: a.binding(\.hints.frequency)) {
                    ForEach(HintFrequency.allCases, id: \.self) { Text($0.title).tag($0) }
                }
                .disabled(!model.settings.hints.enabled)
                SettingsSlider(title: "Stay on screen", value: seconds, range: 1.5...15, step: 0.5) {
                    String(format: "%.1f s", $0)
                }
                .disabled(!model.settings.hints.enabled)
                Picker("Corner", selection: a.binding(\.hints.corner)) {
                    ForEach(Corner.allCases, id: \.self) { Text($0.title).tag($0) }
                }
                .disabled(!model.settings.hints.enabled)
                Toggle("Stop once I've used a shortcut a few times", isOn: a.binding(\.hints.stopWhenLearned))
                    .disabled(!model.settings.hints.enabled)
            }
        }
        .settingsPane()
    }

    private var seconds: Binding<Double> {
        Binding(get: { Double(model.settings.hints.seconds) }, set: { v in
            var s = model.settings
            s.hints.seconds = Float(v)
            a.write(s)
        })
    }
}

#if DEBUG
extension Notification.Name { static let fsDebugRecord = Notification.Name("FSDebugRecord") }
#endif

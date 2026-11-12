// SettingsPanes.swift — General, Appearance, Views & Columns, Address Bar,
// Operations & Safety, Advanced. (Shortcuts and Menus have their own files.)

import AppKit
import SwiftUI

// MARK: - General

struct SettingsGeneralPane: View {
    let model: AppModel
    private var a: SettingsAccess { SettingsAccess(model: model) }

    var body: some View {
        Form {
            Section("New windows") {
                Picker("Default layout", selection: a.binding(\.general.dualPaneByDefault)) {
                    Text("One pane").tag(false)
                    Text("Two panes").tag(true)
                }
                Toggle("Reopen the windows and tabs from last time", isOn: a.binding(\.general.restoreSession))
                LabeledContent("Start folder") {
                    TextField("", text: a.binding(\.general.startFolder), prompt: Text("~"))
                        .labelsHidden().multilineTextAlignment(.trailing).frame(width: 260)
                }
                .help("`~` is your home folder")
                Stepper("Closed tabs remembered: \(model.settings.general.closedTabsKept)",
                        value: a.double(\.general.closedTabsKept).int, in: 1...100)
            }
            Section("Open in Terminal") {
                Picker("Terminal app", selection: a.binding(\.general.terminalApp)) {
                    ForEach(TerminalApp.allCases, id: \.self) { Text($0.title).tag($0) }
                }
                Text("Terminal, iTerm and Ghostty must be installed to be used.").font(.caption).foregroundStyle(.secondary)
            }
            Section("Confirmations") {
                Toggle("Ask before moving to the Trash", isOn: a.binding(\.general.confirmTrash))
                Text("Items always go to the Trash, never straight to deletion, and the Trash keeps them until you empty it. Off by default, as in Finder.")
                    .font(.caption).foregroundStyle(.secondary)
            }
            Section("Typing") {
                SettingsSlider(title: "Type-to-select pause", value: a.double(\.general.typeSelectTimeoutMs),
                               range: 200...5000, step: 100) { String(format: "%.1f s", $0 / 1000) }
            }
            Section("Setup") {
                Button("Run First-Time Setup Again…") { SettingsHooks.rerunFirstRunSetup?() }
                    .disabled(SettingsHooks.rerunFirstRunSetup == nil)
            }
        }
        .settingsPane()
    }
}

extension Binding where Value == Double {
    /// A Double binding as an Int (steppers).
    var int: Binding<Int> { Binding<Int>(get: { Int(wrappedValue) }, set: { wrappedValue = Double($0) }) }
}

// MARK: - Appearance

struct SettingsAppearancePane: View {
    let model: AppModel
    private var a: SettingsAccess { SettingsAccess(model: model) }

    var body: some View {
        Form {
            Section("Lists") {
                Picker("Row density", selection: a.binding(\.view.density)) {
                    Text("Compact (20 pt)").tag(RowDensity.compact)
                    Text("Regular (22 pt)").tag(RowDensity.regular)
                    Text("Comfortable (26 pt)").tag(RowDensity.comfortable)
                }
            }
            Section("Window") {
                Toggle("Show the folder path under each pane's title", isOn: a.bool("appearance.path_bar_subtitle", default: true))
                Picker("Tab bar", selection: a.bool("appearance.tab_bar_always", default: false)) {
                    Text("Only with two or more tabs").tag(false)
                    Text("Always").tag(true)
                }
            }
            Section {
                Picker("Motion", selection: motion) {
                    ForEach(MotionLevel.allCases, id: \.self) { Text($0.title).tag($0) }
                }
                Text("Applies on top of the system's Reduce Motion setting, which always wins. Reduced fades things in and out; None doesn't animate at all.")
                    .font(.caption).foregroundStyle(.secondary)
            } header: { Text("Motion") }
        }
        .settingsPane()
    }

    private var motion: Binding<MotionLevel> {
        let raw = a.string("appearance.motion", default: "Full")
        return Binding(get: { MotionLevel(rawValue: raw.wrappedValue) ?? .full }, set: { raw.wrappedValue = $0.rawValue })
    }
}

// MARK: - Views & Columns

struct SettingsViewsPane: View {
    let model: AppModel
    private var a: SettingsAccess { SettingsAccess(model: model) }

    var body: some View {
        Form {
            Section("New folders open as") {
                Picker("View", selection: a.binding(\.view.mode)) {
                    Text("Icons").tag(ViewMode.icons)
                    Text("List").tag(ViewMode.list)
                    Text("Columns").tag(ViewMode.columns)
                    Text("Gallery").tag(ViewMode.gallery)
                }
                Picker("Sort by", selection: a.binding(\.view.sort)) {
                    Text("Name").tag(SortBy.name)
                    Text("Size").tag(SortBy.size)
                    Text("Date Modified").tag(SortBy.modified)
                    Text("Date Created").tag(SortBy.created)
                    Text("Kind").tag(SortBy.kind)
                    Text("Extension").tag(SortBy.ext)
                }
                Picker("Order", selection: a.binding(\.view.ascending)) {
                    Text("Ascending").tag(true)
                    Text("Descending").tag(false)
                }
                Toggle("Folders first", isOn: a.binding(\.view.foldersFirst))
                Toggle("Treat apps and packages as files when sorting", isOn: a.binding(\.view.packagesAsFiles))
                Toggle("Show hidden files", isOn: a.binding(\.view.showHidden))
            }
            Section("Remembering") {
                Picker("Per-folder view", selection: a.binding(\.view.folderMemory)) {
                    Text("Off — every folder uses these defaults").tag(false)
                    Text("Smart — a folder remembers what you change in it").tag(true)
                }
            }
            Section("Dates") {
                Picker("Clock", selection: a.binding(\.view.hour24)) {
                    Text("24-hour").tag(true)
                    Text("12-hour").tag(false)
                }
            }
        }
        .settingsPane()
    }
}

// MARK: - Address Bar

struct SettingsAddressBarPane: View {
    let model: AppModel
    @State private var newPattern = ""
    private var a: SettingsAccess { SettingsAccess(model: model) }

    var body: some View {
        Form {
            Section("Completion") {
                Stepper("Suggestions shown: \(model.settings.pathBar.completionLimit)",
                        value: a.double(\.pathBar.completionLimit).int, in: 1...500, step: 5)
                Toggle("Suggest hidden folders without typing a dot", isOn: a.binding(\.pathBar.completeHidden))
            }
            Section("Breadcrumbs") {
                Stepper("Segments shown before the middle collapses: \(model.settings.pathBar.maxCrumbs)",
                        value: a.double(\.pathBar.maxCrumbs).int, in: 2...30)
            }
            Section {
                ForEach(model.settings.pathBar.frecencyExcluded, id: \.self) { p in
                    HStack {
                        Text(p).font(.body.monospaced())
                        Spacer()
                        Button { remove(p) } label: { Image(systemName: "minus.circle") }.buttonStyle(.borderless)
                    }
                }
                HStack {
                    TextField("", text: $newPattern, prompt: Text("Add a folder pattern, e.g. /Volumes/Backup/*"))
                        .labelsHidden().onSubmit(add)
                    Button("Add", action: add).disabled(newPattern.trimmingCharacters(in: .whitespaces).isEmpty)
                }
            } header: { Text("Never remember these folders") } footer: {
                Text("Folders matching these patterns never appear in frequent-folder suggestions. `*` matches anything.")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
        .settingsPane()
    }

    private func add() {
        let p = newPattern.trimmingCharacters(in: .whitespaces)
        guard !p.isEmpty else { return }
        var s = model.settings
        if !s.pathBar.frecencyExcluded.contains(p) { s.pathBar.frecencyExcluded.append(p) }
        a.write(s)
        newPattern = ""
    }

    private func remove(_ p: String) {
        var s = model.settings
        s.pathBar.frecencyExcluded.removeAll { $0 == p }
        a.write(s)
    }
}

// MARK: - Operations & Safety

struct SettingsOperationsPane: View {
    let model: AppModel
    @State private var confirmDelete = false
    private var a: SettingsAccess { SettingsAccess(model: model) }

    var body: some View {
        Form {
            Section("Copying") {
                Toggle("Verify copies (re-read and compare a checksum)", isOn: a.binding(\.operations.verifyCopies))
                Toggle("Flush every copied file to disk", isOn: a.binding(\.operations.fsyncCopies))
                Text("Verifying makes copies slower but proves every byte arrived. Moves between disks are always verified before the original goes.")
                    .font(.caption).foregroundStyle(.secondary)
            }
            Section("Speed") {
                Stepper("Files copied at once: \(model.settings.operations.fileWorkers)",
                        value: a.double(\.operations.fileWorkers).int, in: 1...16)
                Stepper("Operations per disk at once: \(model.settings.operations.opsPerVolume)",
                        value: a.double(\.operations.opsPerVolume).int, in: 1...8)
                Text("Changes apply when no operation is running.").font(.caption).foregroundStyle(.secondary)
            }
            Section("Undo") {
                Stepper("Undo steps kept: \(model.settings.operations.undoLimit)",
                        value: a.double(\.operations.undoLimit).int, in: 1...1000, step: 10)
            }
            Section {
                Toggle("Allow “Delete Immediately”", isOn: deleteBinding)
                Label {
                    Text("Delete Immediately removes items for good, with no Trash and no undo. It is off so nothing can be lost by accident. When on, it still asks every time and has no Vim key.")
                        .font(.caption)
                } icon: {
                    Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(.orange)
                }
            } header: { Text("Danger zone") }
        }
        .settingsPane()
        .confirmationDialog("Allow Delete Immediately?", isPresented: $confirmDelete, titleVisibility: .visible) {
            Button("Allow Delete Immediately", role: .destructive) { a.binding(\.operations.allowDeleteImmediately).wrappedValue = true }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Items deleted this way can't be recovered. You will still be asked each time.")
        }
    }

    /// Turning it on asks first; turning it off doesn't.
    private var deleteBinding: Binding<Bool> {
        let real = a.binding(\.operations.allowDeleteImmediately)
        return Binding(get: { real.wrappedValue }, set: { on in
            if on { confirmDelete = true } else { real.wrappedValue = false }
        })
    }
}

// MARK: - Advanced

struct SettingsAdvancedPane: View {
    let model: AppModel
    @State private var confirmReset = false
    private var a: SettingsAccess { SettingsAccess(model: model) }

    /// Mirrors `EngineFactory`'s choice of folder.
    static func dataFolder(_ model: AppModel) -> String? {
        guard model.engine.implementationName != "Mock" else { return nil }
        var dir = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("File Studio", isDirectory: true)
        #if DEBUG
        if let d = UserDefaults.standard.string(forKey: "FSDataDir") { dir = URL(fileURLWithPath: d) }
        #endif
        return dir.path
    }

    var body: some View {
        Form {
            Section("Data") {
                LabeledContent("Data folder") {
                    VStack(alignment: .trailing, spacing: 4) {
                        Text(Self.dataFolder(model) ?? "None (running on sample data)")
                            .font(.callout.monospaced()).textSelection(.enabled).multilineTextAlignment(.trailing)
                        if let d = Self.dataFolder(model) {
                            Button("Show in Finder") { NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: d)]) }
                        }
                    }
                }
                Text("Your settings, history and sessions live here. Your files are never stored in it.")
                    .font(.caption).foregroundStyle(.secondary)
            }
            Section("Troubleshooting") {
                Button("Open Logs in Console") {
                    NSWorkspace.shared.open(URL(fileURLWithPath: "/System/Applications/Utilities/Console.app"))
                }
                Text("In Console, search for “app.filestudio” to see this app's messages.").font(.caption).foregroundStyle(.secondary)
            }
            Section("Reset") {
                Button("Reset All Settings…", role: .destructive) { confirmReset = true }
                Text("Puts every setting, shortcut and context menu back to its default. Your files and history stay.")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
        .settingsPane()
        .confirmationDialog("Reset all settings?", isPresented: $confirmReset, titleVisibility: .visible) {
            Button("Reset Everything", role: .destructive, action: resetAll)
            Button("Cancel", role: .cancel) {}
        } message: { Text("Settings, shortcuts and menus return to their defaults.") }
    }

    private func resetAll() {
        do {
            try model.engine.resetCommandKeys(nil)
            try model.engine.resetMenus()
            var defaults = Settings()
            defaults.pathBar.frecencyExcluded = ["/private/var/folders/*"]   // the core's own default
            try model.engine.updateSettings(defaults)
            try Core.set(model.engine, "appearance.motion", string: "Full")
            try Core.set(model.engine, "appearance.tab_bar_always", bool: false)
            try Core.set(model.engine, "appearance.path_bar_subtitle", bool: true)
            ToastCenter.of(model).show("Settings reset", style: .success)
        } catch { a.fail(error) }
    }
}

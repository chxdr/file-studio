// SettingsFileExplorer.swift — Settings › General: File Studio as the Mac's
// file explorer. The default-viewer switch (Platform/FileViewer.swift) and
// the Open/Save panel jump (Platform/FilePanelJump.swift). The state and its
// wording come from the core.

import AppKit
import SwiftUI

struct SettingsFileExplorerSections: View {
    let model: AppModel
    private var a: SettingsAccess { SettingsAccess(model: model) }

    @State private var status = DefaultFileViewer.status()
    @State private var trusted = FilePanelJump.isTrusted
    @State private var working = false
    @State private var error: String?
    @State private var askAccess = false

    var body: some View {
        Section {
            VStack(alignment: .leading, spacing: 4) {
                Text(status.headline).fontWeight(.medium)
                Text(status.detail).font(.caption).foregroundStyle(.secondary)
            }
            .accessibilityIdentifier("file-viewer-status")
            HStack {
                Button("Make File Studio the Default File Viewer") {
                    run { DefaultFileViewer.makeDefault($0) }
                }
                .disabled(!status.canMakeDefault || working)
                Button("Switch Back to Finder") {
                    run { DefaultFileViewer.restoreFinder($0) }
                }
                .disabled(!status.canRestoreFinder || working)
            }
            if let error { Text(error).font(.caption).foregroundStyle(.red) }
            Text(status.explained).font(.caption).foregroundStyle(.secondary)
            Picker("Folders from other apps open in", selection: a.string("integration.folder_opens_in", default: "NewTab")) {
                Text("A new tab").tag("NewTab")
                Text("A new window").tag("NewWindow")
            }
        } header: { Text("Default file viewer") }

        Section {
            Toggle("Send Open and Save panels to File Studio’s folder", isOn: jumpBinding)
            Text("In any app’s Open or Save panel, press \(shortcut) to go to the folder in File Studio’s active pane (or use Send Open/Save Panel Here in the command palette). Change the shortcut in Settings › Shortcuts.")
                .font(.caption).foregroundStyle(.secondary)
            Toggle("Show a “Go to …” button next to Open and Save panels",
                   isOn: a.bool("integration.panel_jump_button", default: true))
                .disabled(!FilePanelJump.shared.isOn)
            if FilePanelJump.shared.isOn {
                if trusted {
                    Label("Accessibility access is allowed.", systemImage: "checkmark.circle")
                        .font(.caption).foregroundStyle(.secondary)
                } else {
                    HStack {
                        Label("Needs Accessibility access to read and drive other apps’ panels.",
                              systemImage: "exclamationmark.triangle")
                            .font(.caption)
                        Spacer()
                        Button("Open Privacy Settings…") { FilePanelJump.openPrivacySettings() }
                    }
                }
                if let p = FilePanelJump.shared.hotKeyProblem {
                    Text(p).font(.caption).foregroundStyle(.red)
                }
            }
        } header: { Text("Open and Save panels") }
        .alert("Allow File Studio to drive Open and Save panels?", isPresented: $askAccess) {
            Button("Continue") {
                setJump(true)
                FilePanelJump.requestTrust()
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("To send another app’s Open or Save panel to a folder, File Studio needs Accessibility access: it reads which panel is open and types ⌘⇧G, the folder’s path and Return into it, as you would. It does nothing else with the access, and only when you press the shortcut or the button. macOS will ask you to allow it in Privacy & Security › Accessibility.")
        }
        .task {
            // Another app (or the other File Studio) can change these at any time.
            while !Task.isCancelled {
                refresh()
                try? await Task.sleep(for: .seconds(2))
            }
        }
    }

    private var shortcut: String { model.engine.commandKeys("panel.jump_here").first ?? "its shortcut" }

    private var jumpBinding: Binding<Bool> {
        Binding(get: { _ = model.settings; return FilePanelJump.shared.isOn }, set: { on in
            if on && !FilePanelJump.isTrusted { askAccess = true } else { setJump(on) }
        })
    }

    private func setJump(_ on: Bool) {
        do { try Core.set(model.engine, "integration.panel_jump", bool: on) } catch { a.fail(error) }
        refresh()
    }

    private func refresh() {
        status = DefaultFileViewer.status()
        trusted = FilePanelJump.isTrusted
    }

    private func run(_ body: (@escaping @MainActor (Error?) -> Void) -> Void) {
        working = true
        error = nil
        body { err in
            working = false
            error = err.map { "macOS didn’t accept the change: \($0.localizedDescription)" }
            refresh()
        }
    }
}

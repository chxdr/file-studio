// SettingsPlaceholderView.swift — PLACEHOLDER Settings window. The ops-UI
// session builds the real tabs (PROPOSAL §8). It shows the wiring: read
// `engine.settings()`, change a copy, `engine.updateSettings(_:)`; the core
// validates and sends `.settingsChanged`.

import SwiftUI

struct SettingsPlaceholderView: View {
    let model: AppModel

    var body: some View {
        Form {
            Toggle("Show hidden files by default", isOn: binding(\.view.showHidden))
            Toggle("Ask before moving to the Trash", isOn: binding(\.general.confirmTrash))
            Picker("Open in Terminal uses", selection: binding(\.general.terminalApp)) {
                ForEach(TerminalApp.allCases, id: \.self) { Text($0.title).tag($0) }
            }
        }
        .padding(20)
        .frame(width: 420)
    }

    private func binding<T>(_ kp: WritableKeyPath<Settings, T>) -> Binding<T> {
        Binding(get: { model.settings[keyPath: kp] }, set: { v in
            var s = model.settings
            s[keyPath: kp] = v
            do { try model.engine.updateSettings(s) } catch { model.statusMessage = error.localizedDescription }
        })
    }
}

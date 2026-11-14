// SettingsPreviewsPane.swift — Settings → Previews. Every value is a core
// setting (`preview.*`), stored by the core.

import SwiftUI

struct SettingsPreviewsPane: View {
    let model: AppModel
    private var a: SettingsAccess { SettingsAccess(model: model) }

    var body: some View {
        Form {
            Section("Preview pane") {
                Toggle("Show the preview pane", isOn: a.bool("preview.visible", default: false))
                Text("The pane on the right shows a picture and details of the selected item. Toggle it any time with ⌥⌘P or View → Show or Hide Preview.")
                    .font(.caption).foregroundStyle(.secondary)
                Toggle("Show a picture or thumbnail", isOn: a.bool("preview.thumbnails", default: true))
            }
            Section("Text and code") {
                Toggle("Colour code and markup", isOn: a.bool("preview.highlight", default: true))
                SettingsSlider(title: "Lines shown", value: lines, range: 10...1000, step: 10) { "\(Int($0))" }
            }
            Section("iCloud") {
                Text("Files that are only in iCloud are never downloaded to preview them. The pane shows a Download button instead.")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
        .settingsPane()
    }

    /// `preview.max_text_lines`, as a slider value.
    private var lines: Binding<Double> {
        Binding(get: { _ = model.settings; return Double(Core.int(model.engine, "preview.max_text_lines") ?? 200) },
                set: { v in
                    do { try model.engine.setSetting("preview.max_text_lines", json: String(Int(v.rounded()))) } catch { a.fail(error) }
                })
    }
}

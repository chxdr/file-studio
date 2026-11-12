// SettingsRoot.swift — the Settings window: tabs, how it opens, and the small
// helpers every tab uses. Nothing here is stored in UserDefaults: every knob
// reads from and writes to core settings (`engine.settings()` /
// `updateSettings`, `setSetting` by dotted key, the keymap and menu calls).

import AppKit
import Observation
import SwiftUI

enum SettingsTab: String, CaseIterable, Hashable {
    case general, appearance, views, addressBar, operations, shortcuts, menus, advanced
    var title: String {
        switch self {
        case .general: "General"
        case .appearance: "Appearance"
        case .views: "Views & Columns"
        case .addressBar: "Address Bar"
        case .operations: "Operations & Safety"
        case .shortcuts: "Shortcuts"
        case .menus: "Menus"
        case .advanced: "Advanced"
        }
    }
    var symbol: String {
        switch self {
        case .general: "gearshape"
        case .appearance: "paintbrush"
        case .views: "list.bullet.rectangle"
        case .addressBar: "location"
        case .operations: "externaldrive.badge.checkmark"
        case .shortcuts: "keyboard"
        case .menus: "contextualmenu.and.cursorarrow"
        case .advanced: "wrench.and.screwdriver"
        }
    }
    /// The label under the icon in the tab bar.
    var shortTitle: String {
        switch self {
        case .views: "Views"
        case .operations: "Safety"
        default: title
        }
    }
    /// From a `ShellRequest.showSettings(section:)` name.
    init?(section: String) {
        let k = section.lowercased().replacingOccurrences(of: " ", with: "").replacingOccurrences(of: "_", with: "")
        guard let t = Self.allCases.first(where: { $0.rawValue.lowercased() == k }) else { return nil }
        self = t
    }
}

/// Where other code can plug into Settings.
@MainActor
enum SettingsHooks {
    /// "Run First-Time Setup Again…" in General: the window layer sets this
    /// when it has a first-run flow. The button is disabled while it is nil.
    static var rerunFirstRunSetup: (() -> Void)?
}

/// Opens the Settings window, optionally on a tab.
@MainActor
enum SettingsWindow {
    private static var window: NSWindow?
    private static let nav = SettingsNavigation()

    static func show(model: AppModel, section: SettingsTab? = nil) {
        if let section { nav.tab = section }
        if window == nil {
            let w = NSWindow(contentViewController: NSHostingController(rootView: SettingsRoot(model: model, nav: nav)))
            w.title = "Settings"
            w.identifier = NSUserInterfaceItemIdentifier("settings")
            w.styleMask = [.titled, .closable, .miniaturizable]
            w.isReleasedWhenClosed = false
            w.center()
            window = w
        }
        retitle()
        window?.makeKeyAndOrderFront(nil)
        NSApp.activate()
    }

    /// The window is titled with the tab, as the system's Settings is.
    static func retitle() { window?.title = nav.tab.title }
}

@MainActor
@Observable
final class SettingsNavigation { var tab: SettingsTab = .general }

struct SettingsRoot: View {
    let model: AppModel
    let nav: SettingsNavigation

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 2) {
                ForEach(SettingsTab.allCases, id: \.self) { tab in
                    let on = nav.tab == tab
                    Button { nav.tab = tab } label: {
                        VStack(spacing: 3) {
                            Image(systemName: tab.symbol).font(.system(size: 19)).frame(height: 24)
                            Text(tab.shortTitle).font(.caption)
                        }
                        .frame(width: 82, height: 54)
                        .foregroundStyle(on ? Color.accentColor : Color.secondary)
                        .background(RoundedRectangle(cornerRadius: 9).fill(on ? Color.accentColor.opacity(0.16) : .clear))
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel(tab.title)
                    .accessibilityAddTraits(on ? .isSelected : [])
                }
            }
            .padding(.vertical, 8)
            Divider()
            pane(nav.tab).frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .frame(width: 720, height: 640)
        .onChange(of: nav.tab) { SettingsWindow.retitle() }
    }

    @ViewBuilder private func pane(_ t: SettingsTab) -> some View {
        switch t {
        case .general: SettingsGeneralPane(model: model)
        case .appearance: SettingsAppearancePane(model: model)
        case .views: SettingsViewsPane(model: model)
        case .addressBar: SettingsAddressBarPane(model: model)
        case .operations: SettingsOperationsPane(model: model)
        case .shortcuts: SettingsShortcutsPane(model: model)
        case .menus: SettingsMenusPane(model: model)
        case .advanced: SettingsAdvancedPane(model: model)
        }
    }
}

// MARK: - Helpers

/// Bindings into the core's settings. Reads come from `AppModel.settings`
/// (kept current by `.settingsChanged`); writes go through the core, which
/// validates and persists.
@MainActor
struct SettingsAccess {
    let model: AppModel

    func binding<T>(_ kp: WritableKeyPath<Settings, T>) -> Binding<T> {
        Binding(get: { model.settings[keyPath: kp] }, set: { v in
            var s = model.settings
            s[keyPath: kp] = v
            write(s)
        })
    }

    /// A UInt32 setting edited as a Double (sliders).
    func double(_ kp: WritableKeyPath<Settings, UInt32>) -> Binding<Double> {
        Binding(get: { Double(model.settings[keyPath: kp]) }, set: { v in
            var s = model.settings
            s[keyPath: kp] = UInt32(max(0, v.rounded()))
            write(s)
        })
    }

    /// A single setting by dotted key (the `appearance.*` group).
    func string(_ key: String, default d: String) -> Binding<String> {
        Binding(get: { _ = model.settings; return Core.string(model.engine, key) ?? d }, set: { v in
            do { try Core.set(model.engine, key, string: v) } catch { fail(error) }
        })
    }

    func bool(_ key: String, default d: Bool) -> Binding<Bool> {
        Binding(get: { _ = model.settings; return Core.bool(model.engine, key) ?? d }, set: { v in
            do { try Core.set(model.engine, key, bool: v) } catch { fail(error) }
        })
    }

    func write(_ s: Settings) {
        do { try model.engine.updateSettings(s) } catch { fail(error) }
    }

    func fail(_ error: Error) {
        ToastCenter.of(model).show("Couldn’t save that setting", detail: error.localizedDescription, style: .error)
    }
}

/// A slider with a value label.
struct SettingsSlider: View {
    let title: String
    @Binding var value: Double
    let range: ClosedRange<Double>
    var step: Double = 1
    let format: (Double) -> String

    var body: some View {
        LabeledContent(title) {
            HStack {
                Slider(value: $value, in: range, step: step)
                Text(format(value)).monospacedDigit().foregroundStyle(.secondary).frame(width: 64, alignment: .trailing)
            }
            .frame(width: 280)
        }
    }
}

extension View {
    /// The shared look of a settings pane.
    func settingsPane() -> some View { formStyle(.grouped) }
}

extension Settings.Overlay { static let standard = Settings.Overlay() }

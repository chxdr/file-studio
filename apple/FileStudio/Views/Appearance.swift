// Appearance.swift — the look-and-motion settings (`appearance.*` in core
// settings) and the helpers every animated view uses to honour them.
//
// The core only stores these; the shell reads them (any session may use
// `AppearanceModel.of(model)`): `motion` (Full / Reduced / None, on top of the
// system's Reduce Motion), `tabBarAlways`, `pathBarSubtitle`.

import AppKit
import Observation
import SwiftUI

/// How much the interface moves.
enum MotionLevel: String, CaseIterable, Sendable {
    case full = "Full", reduced = "Reduced", none = "None"
    var title: String {
        switch self {
        case .full: "Full"
        case .reduced: "Reduced (fades only)"
        case .none: "None"
        }
    }
}

@MainActor
@Observable
final class AppearanceModel {
    private(set) var motion: MotionLevel = .full
    private(set) var tabBarAlways = false
    private(set) var pathBarSubtitle = true
    @ObservationIgnored private unowned let app: AppModel
    @ObservationIgnored private var token: ObserverToken?

    static func of(_ m: AppModel) -> AppearanceModel { m.attachment(AppearanceModel.self) { AppearanceModel(m) } }

    private init(_ m: AppModel) {
        app = m
        reload()
        token = m.observe { [weak self] e in if case .settingsChanged = e { self?.reload() } }
    }

    func reload() {
        motion = Core.string(app.engine, "appearance.motion").flatMap(MotionLevel.init(rawValue:)) ?? .full
        tabBarAlways = Core.bool(app.engine, "appearance.tab_bar_always") ?? false
        pathBarSubtitle = Core.bool(app.engine, "appearance.path_bar_subtitle") ?? true
    }

    /// The level in force: the system's Reduce Motion caps Full at Reduced.
    var effective: MotionLevel {
        if motion == .full, NSWorkspace.shared.accessibilityDisplayShouldReduceMotion { return .reduced }
        return motion
    }

    /// An animation to use, or nil when nothing should move.
    func animation(_ full: Animation) -> Animation? {
        switch effective {
        case .full: full
        case .reduced: .easeInOut(duration: 0.18)
        case .none: nil
        }
    }

    /// A transition: the full one, a fade, or none.
    func transition(_ full: AnyTransition) -> AnyTransition {
        switch effective {
        case .full: full
        case .reduced: .opacity
        case .none: .identity
        }
    }
}

/// Reading and writing single core settings by dotted key (JSON on the wire).
enum Core {
    static func string(_ e: EngineAPI, _ key: String) -> String? { decode(e, key) }
    static func bool(_ e: EngineAPI, _ key: String) -> Bool? { decode(e, key) }
    static func int(_ e: EngineAPI, _ key: String) -> Int? { decode(e, key) }

    private static func decode<T: Decodable>(_ e: EngineAPI, _ key: String) -> T? {
        guard let json = try? e.setting(key), let data = json.data(using: .utf8) else { return nil }
        return try? JSONDecoder().decode(T.self, from: data)
    }

    static func set(_ e: EngineAPI, _ key: String, string v: String) throws {
        let data = try JSONEncoder().encode(v)
        try e.setSetting(key, json: String(decoding: data, as: UTF8.self))
    }

    static func set(_ e: EngineAPI, _ key: String, bool v: Bool) throws { try e.setSetting(key, json: v ? "true" : "false") }
}

extension Corner {
    var title: String {
        switch self {
        case .topLeft: "Top left"
        case .topRight: "Top right"
        case .bottomLeft: "Bottom left"
        case .bottomRight: "Bottom right"
        }
    }
    var alignment: Alignment {
        switch self {
        case .topLeft: .topLeading
        case .topRight: .topTrailing
        case .bottomLeft: .bottomLeading
        case .bottomRight: .bottomTrailing
        }
    }
    var unitPoint: UnitPoint {
        switch self {
        case .topLeft: .topLeading
        case .topRight: .topTrailing
        case .bottomLeft: .bottomLeading
        case .bottomRight: .bottomTrailing
        }
    }
    var isTop: Bool { self == .topLeft || self == .topRight }
    var isLeading: Bool { self == .topLeft || self == .bottomLeft }
}

extension HintFrequency {
    var title: String {
        switch self {
        case .always: "Every time"
        case .sometimes: "Sometimes"
        case .rarely: "Rarely"
        }
    }
}

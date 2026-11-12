// EngineFactory.swift — the single place that picks the core implementation.

import Foundation
import os

/// The UI never constructs an engine anywhere else.
/// Launch with `-FSMockCore` to run against MockCore (UI work without Rust).
/// `-FSDataDir <folder>` (DEBUG builds) keeps the database out of the user's
/// Application Support, for scripted runs.
@MainActor
func makeEngine() -> EngineAPI {
    if ProcessInfo.processInfo.arguments.contains("-FSMockCore") {
        return MockEngine()
    }
    var dir = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        .appendingPathComponent("File Studio", isDirectory: true)
    #if DEBUG
    if let d = UserDefaults.standard.string(forKey: "FSDataDir") { dir = URL(fileURLWithPath: d) }
    #endif
    do {
        return try RustEngine(dataDir: dir)
    } catch {
        Logger(subsystem: "app.filestudio.mac", category: "core")
            .fault("Rust core failed to start: \(error.localizedDescription, privacy: .public); using MockCore")
        return MockEngine()
    }
}

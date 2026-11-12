// EngineFactory.swift — the single place that picks the core implementation.

import Foundation
import os

/// The name of this build's data folder in Application Support. Debug builds
/// are a separate app ("File Studio Debug", bundle id
/// `app.filestudio.mac.debug`, see `apple/project.yml`), so they never read
/// or write the release app's database.
let appDataFolderName: String = {
    #if DEBUG
    "File Studio Debug"
    #else
    "File Studio"
    #endif
}()

/// The core's data folder: `~/Library/Application Support/<appDataFolderName>`,
/// or `-FSDataDir <folder>` in DEBUG builds (scripted runs use a throwaway
/// folder). Release builds ignore `-FSDataDir`.
func appDataFolder() -> URL {
    #if DEBUG
    if let d = UserDefaults.standard.string(forKey: "FSDataDir") { return URL(fileURLWithPath: d) }
    #endif
    return FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        .appendingPathComponent(appDataFolderName, isDirectory: true)
}

/// The UI never constructs an engine anywhere else.
/// Launch with `-FSMockCore` to run against MockCore (UI work without Rust).
@MainActor
func makeEngine() -> EngineAPI {
    if ProcessInfo.processInfo.arguments.contains("-FSMockCore") {
        return MockEngine()
    }
    let dir = appDataFolder()
    do {
        return try RustEngine(dataDir: dir)
    } catch {
        Logger(subsystem: Bundle.main.bundleIdentifier ?? "app.filestudio.mac", category: "core")
            .fault("Rust core failed to start: \(error.localizedDescription, privacy: .public); using MockCore")
        return MockEngine()
    }
}

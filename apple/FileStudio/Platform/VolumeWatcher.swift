// VolumeWatcher.swift — forwards mount, unmount and rename notifications to
// the core (`volumesChanged`), which re-reads volumes and emits `.volumesChanged`.

import AppKit

@MainActor
final class VolumeWatcher {
    private var tokens: [NSObjectProtocol] = []

    init(engine: EngineAPI) {
        let nc = NSWorkspace.shared.notificationCenter
        for name in [NSWorkspace.didMountNotification, NSWorkspace.didUnmountNotification,
                     NSWorkspace.didRenameVolumeNotification] {
            tokens.append(nc.addObserver(forName: name, object: nil, queue: .main) { _ in
                engine.volumesChanged()
            })
        }
    }

    isolated deinit { tokens.forEach { NSWorkspace.shared.notificationCenter.removeObserver($0) } }
}

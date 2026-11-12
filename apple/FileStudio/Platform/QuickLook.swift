// QuickLook.swift — the Quick Look panel for `ShellRequest.quickLook`.
// Space is a core command; the core says which items, this only shows them.

import AppKit
import QuickLookUI

@MainActor
final class QuickLookController: NSResponder, @preconcurrency QLPreviewPanelDataSource, @preconcurrency QLPreviewPanelDelegate {
    static let shared = QuickLookController()
    private var urls: [URL] = []

    /// Shows the panel for these items, or closes it if it already shows them.
    func toggle(_ paths: [String]) {
        guard let panel = QLPreviewPanel.shared() else { return }
        let new = paths.map { URL(fileURLWithPath: $0) }
        if panel.isVisible, new == urls {
            panel.orderOut(nil)
            return
        }
        urls = new
        panel.dataSource = self
        panel.delegate = self
        panel.reloadData()
        panel.makeKeyAndOrderFront(nil)
    }

    func numberOfPreviewItems(in panel: QLPreviewPanel!) -> Int { urls.count }
    func previewPanel(_ panel: QLPreviewPanel!, previewItemAt index: Int) -> (any QLPreviewItem)! {
        urls[index] as NSURL
    }
    /// Keys typed while the panel is key go back to the main window's key routing.
    func previewPanel(_ panel: QLPreviewPanel!, handle event: NSEvent!) -> Bool {
        guard event.type == .keyDown, let model = AppModel.current, let chord = KeyChord(event: event) else { return false }
        let w = NSApp.windows.first { $0.windowController is CoreWindowIdentifying && $0.isVisible }
        let (ctx, wid) = KeyRouter.context(in: w)
        return model.handleKey(chord, context: ctx, window: wid)
    }
}

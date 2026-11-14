// PreviewController.swift — the preview pane as a window's right-hand
// inspector (NSSplitViewItem(inspectorWithViewController:), which gets the
// system's inspector look on macOS 26). One per window; it follows the active
// tab's cursor. Visibility and width live in the core's `preview.*` settings.

import AppKit
import SwiftUI

@MainActor
final class PreviewController: NSViewController {
    let model: PreviewModel
    private unowned let appModel: AppModel
    private let host: NSHostingView<PreviewPanelRoot>

    init(appModel: AppModel) {
        self.appModel = appModel
        model = PreviewModel(previewer: appModel.engine.makePreviewer())
        host = NSHostingView(rootView: PreviewPanelRoot(model: model, appModel: appModel))
        super.init(nibName: nil, bundle: nil)
        syncSettings()
    }
    required init?(coder: NSCoder) { fatalError() }

    override func loadView() {
        host.sizingOptions = []
        view = host
    }

    /// Reads `preview.*` from the core.
    func syncSettings() {
        let e = appModel.engine
        model.visible = Core.bool(e, "preview.visible") ?? model.visible
        model.showThumbnails = Core.bool(e, "preview.thumbnails") ?? true
        model.highlight = Core.bool(e, "preview.highlight") ?? true
    }

    /// The width setting, points.
    var storedWidth: CGFloat { CGFloat(Core.int(appModel.engine, "preview.width") ?? 300) }

    /// Follows `tab`'s cursor: the cursor item, else the only selected item,
    /// else the folder itself.
    func follow(_ tab: TabModel?) {
        guard let tab else { model.follow(path: nil, selectionCount: 0); return }
        let sel = tab.api.selection()
        var path: String?
        if let c = sel.cursor ?? sel.rows.first { path = tab.api.pathAt(c) }
        model.follow(path: path ?? tab.location, selectionCount: sel.rows.count)
    }

    override func viewDidLayout() {
        super.viewDidLayout()
        model.pictureWidth = max(120, view.bounds.width - 28)
        model.scale = view.window?.backingScaleFactor ?? 2
    }
}

/// The SwiftUI root: re-reads the 24-hour setting when settings change.
struct PreviewPanelRoot: View {
    let model: PreviewModel
    let appModel: AppModel
    var body: some View {
        PreviewPanel(model: model, hour24: appModel.settings.view.hour24)
            .background(.background)
    }
}

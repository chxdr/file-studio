// PreviewDebugSteps.swift — DEBUG-only script steps for the preview pane
// (registered with DebugAutomation):
//
//   previewlog              a PREVIEW line: visible, collapsed, width, the item shown,
//                           category, camera, lines, summaries made, selection count
//   previewwait[:<s>]       waits (default 1 s) then logs, so a thumbnail or summary can arrive
//   previewsnap:<name>      draws the pane alone to <SnapshotDir>/<name>.png (window snaps leave it blank)
//   previewset:<key>=<json> sets a preview.* setting (previewset:max_text_lines=20)
//   previewtime:<path>      times the core's summary of a file, three times (PREVIEWTIME lines):
//                           first run and the cached runs
//   previewscroll:<n>       presses ↓ n times with no pause, as holding the key does,
//                           then logs how many summaries were actually made

#if DEBUG
import AppKit
import SwiftUI

@MainActor
enum PreviewDebugSteps {
    private static var installed = false

    static func install() {
        guard !installed else { return }
        installed = true
        DebugAutomation.register("previewlog") { _, _ in log() }
        DebugAutomation.register("previewsnap") { arg, m in
            // SwiftUI inside the inspector doesn't render through cacheDisplay, so the panel is drawn
            // with ImageRenderer from the same model (the picture and details are the live ones).
            guard let c = controller else { return }
            let parts = arg.split(separator: ",").map(String.init)
            let name = parts.first ?? "preview"
            let dark = parts.contains("dark")
            let w = max(240, c.debugPreviewView.bounds.width)
            let view = PreviewPanel(model: c.debugPreviewModel, hour24: true, scrolls: false)
                .frame(width: w, height: 760)
                .background(Color(nsColor: .windowBackgroundColor))
                .environment(\.colorScheme, dark ? .dark : .light)
            let r = ImageRenderer(content: view)
            r.scale = 2
            let dir = URL(fileURLWithPath: UserDefaults.standard.string(forKey: "FSDebugSnapshotDir") ?? NSTemporaryDirectory())
            guard let img = r.nsImage, let tiff = img.tiffRepresentation, let rep = NSBitmapImageRep(data: tiff) else { return }
            let url = dir.appendingPathComponent("\(name).png")
            try? rep.representation(using: .png, properties: [:])?.write(to: url)
            print("PREVIEWSNAP \(url.path)")
        }
        DebugAutomation.register("previewset") { arg, m in
            let parts = arg.split(separator: "=", maxSplits: 1).map(String.init)
            guard parts.count == 2 else { return }
            do { try m.engine.setSetting("preview." + parts[0], json: parts[1]) } catch { print("DEBUG \(error)") }
        }
        DebugAutomation.register("previewtime") { path, m in
            let p = m.engine.makePreviewer()
            for run in ["first", "cached", "cached"] {
                let t0 = DispatchTime.now().uptimeNanoseconds
                let info = p.summarize(path, cancel: PreviewCancel())
                let ms = Double(DispatchTime.now().uptimeNanoseconds - t0) / 1e6
                print("PREVIEWTIME \(run) \(String(format: "%.2f", ms)) ms \(info?.photo?.camera ?? info?.video?.codec ?? info?.text?.language ?? "-")")
            }
        }
        DebugAutomation.extraSteps["previewwait"] = { arg in
            try? await Task.sleep(for: .seconds(Double(arg) ?? 1))
            log()
        }
        DebugAutomation.extraSteps["previewscroll"] = { arg in
            guard let c = controller else { return }
            let before = c.debugPreview["requests"] as? Int ?? 0
            for _ in 0..<(Int(arg) ?? 20) { DebugAutomation.sendKey("down") }
            try? await Task.sleep(for: .seconds(1))
            let made = (c.debugPreview["requests"] as? Int ?? 0) - before
            print("PREVIEWSCROLL keys=\(Int(arg) ?? 20) summaries=\(made)")
            log()
        }
    }

    private static var controller: MainWindowController? {
        (NSApp.keyWindow?.windowController as? MainWindowController)
            ?? NSApp.windows.compactMap { $0.windowController as? MainWindowController }.first
    }

    private static func log() {
        guard let d = controller?.debugPreview,
              let data = try? JSONSerialization.data(withJSONObject: d, options: [.sortedKeys]) else { return }
        print("PREVIEW " + String(decoding: data, as: UTF8.self))
    }
}
#endif

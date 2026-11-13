// ConflictSheet.swift — "an item with that name already exists". A sheet on
// the window shows both items side by side (icon or Quick Look thumbnail,
// name, size, date, kind, which is newer/larger) and asks what to do. The
// core decides what each choice means (`resolveConflict`).
//
// Keys: Return = Keep Both (the safest), ⌘R Replace, S or Esc Skip, M Merge,
// ⌘Y Compare in Quick Look.

import AppKit
import QuickLookThumbnailing
import SwiftUI
import UniformTypeIdentifiers

struct ConflictSheetView: View {
    let ops: OpsModel
    @State private var applyToAll = false

    var body: some View {
        if let c = ops.pending.first {
            ConflictCard(conflict: c, answered: ops.conflictsAnswered, total: ops.conflictsTotal,
                         remaining: ops.pending.count, applyToAll: $applyToAll) { d in
                ops.resolve(c, d, applyToAll: applyToAll)
                applyToAll = false
            }
            .onChange(of: c.id) { applyToAll = false }
        } else {
            Color.clear.frame(width: 1, height: 1)
        }
    }
}

/// The sheet's content for one conflict (also drawn by the power tour's demo).
struct ConflictCard: View {
    let conflict: Conflict
    /// "2 of 5" when more than one conflict is in this run.
    let answered: Int
    let total: Int
    /// Conflicts waiting, this one included.
    let remaining: Int
    @Binding var applyToAll: Bool
    let decide: (ConflictDecision) -> Void
    /// Drawing only (the power tour): no key shortcuts, so the window it sits
    /// in keeps its own Return and Esc; Keep Both keeps the default-button look.
    var drawingOnly = false

    var body: some View {
        let c = conflict
        let name = (c.dest as NSString).lastPathComponent
        VStack(alignment: .leading, spacing: 14) {
            HStack(alignment: .firstTextBaseline) {
                Text("“\(name)” already exists").font(.headline).lineLimit(2)
                Spacer()
                if total > 1 {
                    Text("\(answered + 1) of \(total)")
                        .font(.callout.monospacedDigit()).foregroundStyle(.secondary)
                }
            }
            HStack(alignment: .top, spacing: 12) {
                ItemCard(title: "Incoming", path: c.source, kind: c.sourceKind, size: c.sourceSize, mtimeNs: c.sourceMtimeNs,
                         other: (c.destKind, c.destSize, c.destMtimeNs))
                Image(systemName: "arrow.left.arrow.right").foregroundStyle(.secondary).padding(.top, 60)
                ItemCard(title: "Already here", path: c.dest, kind: c.destKind, size: c.destSize, mtimeNs: c.destMtimeNs,
                         other: (c.sourceKind, c.sourceSize, c.sourceMtimeNs))
            }
            HStack {
                Toggle(remaining > 1 ? "Apply to all (\(remaining - 1) more)" : "Apply to all", isOn: $applyToAll)
                    .shortcut("a", [], unless: drawingOnly)
                Spacer()
                Button("Compare") { QuickLookController.shared.toggle([c.source, c.dest]) }
                    .shortcut("y", .command, unless: drawingOnly)
                    .help("Open both in Quick Look (⌘Y)")
            }
            Divider()
            HStack {
                Text("Replace moves the old item to the Trash.").font(.caption).foregroundStyle(.secondary)
                Spacer()
                Button("Skip") { decide(.skip) }.shortcut("s", [], unless: drawingOnly)
                if c.canMerge { Button("Merge") { decide(.merge) }.shortcut("m", [], unless: drawingOnly) }
                Button("Replace") { decide(.replace) }.shortcut("r", .command, unless: drawingOnly)
                if drawingOnly {
                    Button("Keep Both") {}.buttonStyle(.borderedProminent)
                } else {
                    Button("Keep Both") { decide(.keepBoth) }.keyboardShortcut(.defaultAction)
                }
            }
            // Esc skips, too.
            if !drawingOnly {
                Button("") { decide(.skip) }.keyboardShortcut(.cancelAction).opacity(0).frame(width: 0, height: 0)
            }
        }
        .padding(20)
        .frame(width: 600)
        .id(c.id)
    }
}

private extension View {
    /// A key shortcut, left off when the view only draws.
    @ViewBuilder func shortcut(_ key: KeyEquivalent, _ mods: EventModifiers, unless off: Bool) -> some View {
        if off { self } else { keyboardShortcut(key, modifiers: mods) }
    }
}

private struct ItemCard: View {
    let title: String
    let path: String
    let kind: EntryKind
    let size: UInt64
    let mtimeNs: Int64
    let other: (EntryKind, UInt64, Int64)

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(title.uppercased()).font(.caption2.weight(.semibold)).foregroundStyle(.secondary)
            HStack(alignment: .top, spacing: 12) {
                FileThumbnail(path: path).frame(width: 72, height: 72)
                VStack(alignment: .leading, spacing: 3) {
                    Text((path as NSString).lastPathComponent).font(.system(size: 13, weight: .medium))
                        .lineLimit(2).truncationMode(.middle)
                    Text(Self.kindText(path, kind)).font(.caption).foregroundStyle(.secondary)
                    Text((path as NSString).deletingLastPathComponent.abbreviatingWithTildeInPath)
                        .font(.caption2).foregroundStyle(.tertiary).lineLimit(1).truncationMode(.head)
                }
            }
            Divider()
            fact("Size", kind == .file ? OpsFormat.bytes(size) : "—", tag: sizeTag)
            fact("Modified", date, tag: dateTag)
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 10).fill(Color.primary.opacity(0.05)))
        .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(Color.primary.opacity(0.1)))
    }

    private func fact(_ label: String, _ value: String, tag: (String, Color)?) -> some View {
        HStack {
            Text(label).font(.caption).foregroundStyle(.secondary).frame(width: 58, alignment: .leading)
            Text(value).font(.caption).monospacedDigit().lineLimit(1)
            if let tag {
                Text(tag.0).font(.system(size: 9, weight: .semibold)).foregroundStyle(tag.1)
                    .padding(.horizontal, 5).padding(.vertical, 1)
                    .background(Capsule().fill(tag.1.opacity(0.15)))
            }
        }
    }

    private var date: String {
        mtimeNs == 0 ? "—" : Date(timeIntervalSince1970: Double(mtimeNs) / 1e9).formatted(date: .abbreviated, time: .shortened)
    }
    private var dateTag: (String, Color)? {
        guard mtimeNs != 0, other.2 != 0, mtimeNs != other.2 else { return nil }
        return mtimeNs > other.2 ? ("NEWER", .green) : ("OLDER", .secondary)
    }
    private var sizeTag: (String, Color)? {
        guard kind == .file, other.0 == .file, size != other.1 else { return nil }
        return size > other.1 ? ("LARGER", .blue) : ("SMALLER", .secondary)
    }

    /// The system's own description of the item's type (metadata only).
    static func kindText(_ path: String, _ kind: EntryKind) -> String {
        if let d = try? URL(fileURLWithPath: path).resourceValues(forKeys: [.localizedTypeDescriptionKey]).localizedTypeDescription {
            return d
        }
        // Gone (or not readable): describe it by its extension.
        if kind == .file, let d = UTType(filenameExtension: (path as NSString).pathExtension)?.localizedDescription {
            return d.prefix(1).uppercased() + d.dropFirst()
        }
        switch kind {
        case .dir: return "Folder"
        case .symlink: return "Alias"
        case .file: return "Document"
        case .other: return "Item"
        }
    }
}

/// The item's icon, upgraded to a Quick Look thumbnail when one is ready.
/// Never reads an iCloud placeholder's content (F-016): those keep the icon.
private struct FileThumbnail: View {
    let path: String
    @State private var image: NSImage?

    var body: some View {
        Image(nsImage: image ?? Self.icon(path))
            .resizable().scaledToFit()
            .clipShape(RoundedRectangle(cornerRadius: 6))
            .task(id: path) { image = await Self.thumbnail(path) }
    }

    /// The item's icon, or its type's icon when it is gone.
    private static func icon(_ path: String) -> NSImage {
        if FileManager.default.fileExists(atPath: path) { return NSWorkspace.shared.icon(forFile: path) }
        let ext = (path as NSString).pathExtension
        return ext.isEmpty ? NSWorkspace.shared.icon(for: .data) : NSWorkspace.shared.icon(for: UTType(filenameExtension: ext) ?? .data)
    }

    private static func isDataless(_ path: String) -> Bool {
        var st = stat()
        guard lstat(path, &st) == 0 else { return false }
        return st.st_flags & 0x4000_0000 != 0   // SF_DATALESS
    }

    private static func thumbnail(_ path: String) async -> NSImage? {
        guard !isDataless(path) else { return nil }
        var isDir: ObjCBool = false
        guard FileManager.default.fileExists(atPath: path, isDirectory: &isDir), !isDir.boolValue else { return nil }
        let req = QLThumbnailGenerator.Request(fileAt: URL(fileURLWithPath: path), size: CGSize(width: 96, height: 96),
                                               scale: 2, representationTypes: .thumbnail)
        let rep = try? await QLThumbnailGenerator.shared.generateBestRepresentation(for: req)
        return rep?.nsImage
    }
}

private extension String {
    var abbreviatingWithTildeInPath: String { (self as NSString).abbreviatingWithTildeInPath }
}

// MARK: - Presenter

/// Shows the sheet on the front window while conflicts are pending.
@MainActor
final class ConflictPresenter {
    static let shared = ConflictPresenter()
    private var ops: OpsModel!
    private var sheet: NSWindow?
    private weak var parent: NSWindow?

    func install(_ model: AppModel) {
        guard ops == nil else { return }
        ops = OpsModel.of(model)
        observe()
        update()
    }

    private func observe() {
        withObservationTracking { _ = ops.pending.count } onChange: { [weak self] in
            Task { @MainActor in
                self?.update()
                self?.observe()
            }
        }
    }

    private func update() {
        if ops.pending.isEmpty {
            if let s = sheet { parent?.endSheet(s); s.orderOut(nil); sheet = nil }
            return
        }
        guard sheet == nil else { return }
        let host = NSApp.keyWindow.flatMap { $0.windowController is CoreWindowIdentifying ? $0 : nil }
            ?? NSApp.windows.first { $0.windowController is CoreWindowIdentifying && $0.isVisible }
        guard let host else { return }
        if host.attachedSheet != nil {   // another sheet is up: try again shortly
            Task { @MainActor [weak self] in
                try? await Task.sleep(for: .milliseconds(400))
                self?.update()
            }
            return
        }
        let vc = NSHostingController(rootView: ConflictSheetView(ops: ops))
        let w = NSWindow(contentViewController: vc)
        w.styleMask = [.titled]
        w.identifier = NSUserInterfaceItemIdentifier("conflict-sheet")
        sheet = w
        parent = host
        host.beginSheet(w) { [weak self] _ in self?.sheet = nil }
    }
}

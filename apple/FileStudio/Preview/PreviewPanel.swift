// PreviewPanel.swift — the right-hand preview pane: a picture (or the file's
// icon), the name and kind, then a clean list of details in sections, in the
// style of ForkLift's inspector: small grey labels, values you can select.
// iCloud placeholders get a "Not downloaded" card with a Download button.

import SwiftUI
import UniformTypeIdentifiers

struct PreviewPanel: View {
    let model: PreviewModel
    let hour24: Bool
    /// False only for debug snapshots: ImageRenderer can't draw scroll views.
    var scrolls = true

    var body: some View {
        Group {
            if let info = model.info {
                content(info)
            } else if model.loading {
                ProgressView().controlSize(.small).frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                emptyState
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Preview")
    }

    private var emptyState: some View {
        VStack(spacing: 6) {
            Image(systemName: "doc.text.magnifyingglass").font(.system(size: 30, weight: .light))
            Text("Nothing to preview").font(.callout)
        }
        .foregroundStyle(.tertiary)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    @ViewBuilder private func content(_ i: PreviewInfo) -> some View {
        let inner = VStack(alignment: .leading, spacing: 14) {
            hero(i)
            title(i)
            if i.dataless { placeholder(i) }
            if let t = i.text, !i.dataless { TextSection(text: t, scrolls: scrolls) }
            details(i)
        }
        .padding(14)
        if scrolls { ScrollView { inner }.scrollIndicators(.automatic) } else { inner }
    }

    // MARK: Picture

    private func hero(_ i: PreviewInfo) -> some View {
        ZStack {
            if let p = model.picture, !i.dataless {
                Image(nsImage: p).resizable().scaledToFit()
                    .clipShape(RoundedRectangle(cornerRadius: 6, style: .continuous))
                    .shadow(color: .black.opacity(0.18), radius: 4, y: 1)
                    .accessibilityLabel("Picture of \(i.name)")
            } else if let icon = model.icon {
                Image(nsImage: icon).resizable().scaledToFit()
                    .frame(width: 80, height: 80)
                    .opacity(i.dataless ? 0.5 : 1)
                    .accessibilityHidden(true)
            }
        }
        .frame(maxWidth: .infinity, minHeight: 80, maxHeight: 220)
    }

    private func title(_ i: PreviewInfo) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(i.name).font(.headline).textSelection(.enabled).lineLimit(3)
                .frame(maxWidth: .infinity, alignment: .leading)
            HStack(spacing: 6) {
                Text(PreviewFormat.kind(i)).foregroundStyle(.secondary)
                if model.selectionCount > 1 {
                    Text("· \(model.selectionCount) selected").foregroundStyle(.tertiary)
                }
            }
            .font(.callout)
        }
    }

    // MARK: Placeholder

    private func placeholder(_ i: PreviewInfo) -> some View {
        let busy = model.downloading.contains(i.path)
        return VStack(spacing: 8) {
            Label("Not downloaded", systemImage: "icloud.and.arrow.down")
                .font(.callout.weight(.medium))
            Text("This file is in iCloud and isn’t on this Mac. Nothing has been fetched to show it.")
                .font(.caption).foregroundStyle(.secondary).multilineTextAlignment(.center)
            Button(busy ? "Downloading…" : "Download") { model.download() }
                .buttonStyle(.borderedProminent).controlSize(.regular).disabled(busy)
        }
        .padding(12)
        .frame(maxWidth: .infinity)
        .background(RoundedRectangle(cornerRadius: 10, style: .continuous).fill(.quaternary.opacity(0.5)))
    }

    // MARK: Details

    @ViewBuilder private func details(_ i: PreviewInfo) -> some View {
        Section2("General") {
            if i.category != .folder && i.category != .package { row("Size", PreviewFormat.size(i.size)) }
            row("Modified", PreviewFormat.date(i.modifiedNs, hour24: hour24))
            if let c = i.createdNs { row("Created", PreviewFormat.date(c, hour24: hour24)) }
            if let f = i.folder, !i.dataless { row("Items", PreviewFormat.items(f)) }
            if i.isSymlink { row("Link", "Shows the item it points to") }
        }
        if let p = i.photo, !i.dataless {
            Section2("Camera") {
                opt("Taken", p.dateTaken)
                opt("Camera", p.camera)
                opt("Lens", p.lens)
                opt("Focal length", p.focalLength)
                opt("Aperture", p.aperture)
                opt("Shutter", p.shutter)
                opt("ISO", p.iso.map { $0.replacingOccurrences(of: "ISO ", with: "") })
                opt("Exposure", p.exposureComp)
                opt("Dimensions", p.dimensions)
            }
        }
        if let v = i.video, !i.dataless {
            Section2("Video") {
                opt("Codec", v.codec)
                opt("Duration", v.duration)
                opt("Resolution", v.resolution)
                opt("Frame rate", v.frameRate)
                opt("Audio", v.audio)
            }
        }
        if let t = i.text, !i.dataless {
            Section2("Text") {
                row("Lines", (t.linesExact ? "" : "about ") + t.lineCount.formatted())
                row("Encoding", t.encoding)
                opt("Language", t.language)
            }
        }
        if let m = i.marks {
            Section2("Culling") {
                CullRows(marks: m)
            }
        }
        if !i.tags.isEmpty {
            Section2("Tags") {
                FlowTags(tags: i.tags)
            }
        }
        Section2("Where") {
            Text(i.path).font(.callout).textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        if let n = i.note, !i.dataless {
            Text(n).font(.caption).foregroundStyle(.secondary)
        }
    }

    private func row(_ label: String, _ value: String) -> some View { InfoRow(label: label, value: value) }

    @ViewBuilder private func opt(_ label: String, _ value: String?) -> some View {
        if let value, !value.isEmpty { InfoRow(label: label, value: value) }
    }
}

// MARK: - Pieces

/// A section: a small caps title, a hairline, then rows.
private struct Section2<Content: View>: View {
    let title: String
    @ViewBuilder let content: Content
    init(_ title: String, @ViewBuilder content: () -> Content) { self.title = title; self.content = content() }

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            Text(title).font(.caption.weight(.semibold)).foregroundStyle(.secondary).textCase(.uppercase)
            Divider()
            content
        }
    }
}

private struct InfoRow: View {
    let label: String
    let value: String
    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Text(label).foregroundStyle(.secondary).frame(width: 82, alignment: .trailing)
            Text(value).textSelection(.enabled).fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .font(.callout)
        .accessibilityElement(children: .combine)
    }
}

/// Stars, colour label and pick/reject from the XMP sidecar (display only).
private struct CullRows: View {
    let marks: CullMarks
    private var labelName: String? {
        switch marks.label {
        case .red?: "Red"
        case .yellow?: "Yellow"
        case .green?: "Green"
        case .blue?: "Blue"
        case .purple?: "Purple"
        case nil: nil
        }
    }
    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text("Rating").foregroundStyle(.secondary).frame(width: 82, alignment: .trailing)
                if marks.rating == 0 {
                    Text("Unrated")
                } else {
                    Text(String(repeating: "★", count: Int(min(marks.rating, 5)))
                         + String(repeating: "☆", count: 5 - Int(min(marks.rating, 5))))
                        .foregroundStyle(.orange.opacity(marks.flag == .reject ? 0.5 : 1))
                }
                Spacer(minLength: 0)
            }
            .accessibilityElement(children: .ignore)
            .accessibilityLabel("Rating: \(marks.rating == 0 ? "unrated" : "\(marks.rating) stars")")
            if let l = marks.label, let name = labelName {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text("Label").foregroundStyle(.secondary).frame(width: 82, alignment: .trailing)
                    Circle().fill(Color(nsColor: CellStyle.color(l))).frame(width: 9, height: 9)
                    Text(name)
                    Spacer(minLength: 0)
                }
                .accessibilityElement(children: .combine)
            }
            if marks.flag != .none {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text("Flag").foregroundStyle(.secondary).frame(width: 82, alignment: .trailing)
                    Image(systemName: marks.flag == .pick ? "flag.fill" : "xmark.circle.fill")
                        .foregroundStyle(marks.flag == .pick ? .green : .red)
                    Text(marks.flag == .pick ? "Picked" : "Rejected")
                    Spacer(minLength: 0)
                }
                .accessibilityElement(children: .combine)
            }
        }
        .font(.callout)
    }
}

private struct FlowTags: View {
    let tags: [PreviewTag]
    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            ForEach(Array(tags.enumerated()), id: \.offset) { _, t in
                HStack(spacing: 6) {
                    Circle().fill(PreviewFormat.tagColor(t.color)).frame(width: 9, height: 9)
                    Text(t.name).font(.callout)
                }
            }
        }
        .accessibilityElement(children: .combine)
    }
}

/// The first lines of a text file with line numbers, highlighted.
private struct TextSection: View {
    let text: PreviewText
    var scrolls = true
    private static let font = Font.system(size: 11, design: .monospaced)

    var body: some View {
        if text.lines.isEmpty {
            Text("Empty file").font(.callout).foregroundStyle(.secondary)
        } else {
            VStack(alignment: .leading, spacing: 4) {
                let block = HStack(alignment: .top, spacing: 8) {
                    Text(numbers).font(Self.font).foregroundStyle(.tertiary).multilineTextAlignment(.trailing)
                    Text(code).fixedSize(horizontal: true, vertical: false)
                }
                .padding(8)
                .textSelection(.enabled)
                Group { if scrolls { ScrollView(.horizontal) { block } } else { block.frame(maxWidth: .infinity, alignment: .leading) } }
                .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(.quaternary.opacity(0.45)))
                if text.truncated {
                    Text("Showing the first \(text.lines.count) of \(text.lineCount.formatted()) lines")
                        .font(.caption).foregroundStyle(.secondary)
                }
            }
            .accessibilityElement(children: .contain)
            .accessibilityLabel("Start of the file")
        }
    }

    private var numbers: String { (1...max(1, text.lines.count)).map(String.init).joined(separator: "\n") }

    private var code: AttributedString {
        var out = AttributedString()
        for (n, line) in text.lines.enumerated() {
            if n > 0 { out.append(AttributedString("\n")) }
            out.append(SyntaxTheme.attributed(line, font: Self.font))
        }
        return out
    }
}

// MARK: - Formatting (display only; the numbers come from the core)

enum PreviewFormat {
    static func size(_ bytes: UInt64) -> String {
        if bytes < 1000 { return "\(bytes) bytes" }
        let f = ByteCountFormatter()
        f.countStyle = .file
        return f.string(fromByteCount: Int64(bytes))
    }

    static func date(_ ns: Int64, hour24: Bool) -> String {
        let d = Date(timeIntervalSince1970: TimeInterval(ns) / 1e9)
        let f = DateFormatter()
        f.setLocalizedDateFormatFromTemplate(hour24 ? "d MMM yyyy HH:mm" : "d MMM yyyy h:mm a")
        return f.string(from: d)
    }

    static func items(_ f: PreviewFolder) -> String {
        if f.unreadable { return "Can’t be read" }
        return (f.atLeast ? "more than " : "") + "\(f.items.formatted()) item\(f.items == 1 ? "" : "s")"
    }

    /// "Sony RAW image", "Folder", "JPEG image" — the system's own wording by extension.
    static func kind(_ i: PreviewInfo) -> String {
        switch i.category {
        case .folder: return "Folder"
        case .package where URL(fileURLWithPath: i.path).pathExtension == "app": return "Application"
        default: break
        }
        let ext = URL(fileURLWithPath: i.path).pathExtension
        if let t = UTType(filenameExtension: ext), let d = t.localizedDescription { return d.prefix(1).uppercased() + d.dropFirst() }
        return switch i.category {
        case .image: "Image"
        case .raw: "Camera raw"
        case .video: "Movie"
        case .audio: "Audio"
        case .text: "Text"
        case .package: "Package"
        default: ext.isEmpty ? "Document" : "\(ext.uppercased()) file"
        }
    }

    /// Finder's tag colours.
    static func tagColor(_ i: Int) -> Color {
        switch i {
        case 1: .gray
        case 2: .green
        case 3: .purple
        case 4: .blue
        case 5: .yellow
        case 6: .red
        case 7: .orange
        default: Color.secondary.opacity(0.5)
        }
    }
}
